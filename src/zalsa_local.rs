use std::cell::{RefCell, UnsafeCell};
use std::fmt;
use std::fmt::Formatter;
use std::panic::UnwindSafe;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use rustc_hash::FxHashMap;
use thin_vec::ThinVec;

#[cfg(feature = "accumulator")]
use crate::accumulator::{
    Accumulator,
    accumulated_map::{AccumulatedMap, AtomicInputAccumulatedValues},
};
use crate::active_query::{CompletedQuery, QueryStack};
use crate::cycle::{AtomicIterationCount, CycleHeads, IterationCount, empty_cycle_heads};
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::runtime::Stamp;
use crate::sync::atomic::AtomicBool;
use crate::table::{PageIndex, Slot, Table};
use crate::tracked_struct::{Disambiguator, Identity, IdentityHash};
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Cancelled, Id, Revision};

/// State that is specific to a single execution thread.
///
/// Internally, this type uses ref-cells.
///
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
pub struct ZalsaLocal {
    /// Vector of active queries.
    ///
    /// Unwinding note: pushes onto this vector must be popped -- even
    /// during unwinding.
    query_stack: RefCell<QueryStack>,

    /// Stores the most recent page for a given ingredient.
    /// This is thread-local to avoid contention.
    most_recent_pages: UnsafeCell<FxHashMap<IngredientIndex, PageIndex>>,

    cancelled: CancellationToken,
}

/// A cancellation token that can be used to cancel a query computation for a specific local `Database`.
#[derive(Default, Clone, Debug)]
pub struct CancellationToken(Arc<AtomicU8>);

impl CancellationToken {
    const CANCELLED_MASK: u8 = 0b01;
    const DISABLED_MASK: u8 = 0b10;

    /// Inform the database to cancel the current query computation.
    pub fn cancel(&self) {
        self.0.fetch_or(Self::CANCELLED_MASK, Ordering::Relaxed);
    }

    /// Check if the query computation has been requested to be cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed) & Self::CANCELLED_MASK != 0
    }

    #[inline]
    fn set_cancellation_disabled(&self, disabled: bool) -> bool {
        let previous_disabled_bit = if disabled {
            self.0.fetch_or(Self::DISABLED_MASK, Ordering::Relaxed)
        } else {
            self.0.fetch_and(!Self::DISABLED_MASK, Ordering::Relaxed)
        };
        previous_disabled_bit & Self::DISABLED_MASK != 0
    }

    fn should_trigger_local_cancellation(&self) -> bool {
        self.0.load(Ordering::Relaxed) == Self::CANCELLED_MASK
    }

    fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

impl ZalsaLocal {
    pub(crate) fn new() -> Self {
        ZalsaLocal {
            query_stack: RefCell::new(QueryStack::default()),
            most_recent_pages: UnsafeCell::new(FxHashMap::default()),
            cancelled: CancellationToken::default(),
        }
    }

    pub(crate) fn record_unfilled_pages(&mut self, table: &Table) {
        let most_recent_pages = self.most_recent_pages.get_mut();
        most_recent_pages
            .drain()
            .for_each(|(ingredient, page)| table.record_unfilled_page(ingredient, page));
    }

    /// Allocate a new id in `table` for the given ingredient
    /// storing `value`. Remembers the most recent page from this
    /// thread and attempts to reuse it.
    pub(crate) fn allocate<'db, T: Slot>(
        &self,
        zalsa: &'db Zalsa,
        ingredient: IngredientIndex,
        mut value: impl FnOnce(Id) -> T,
    ) -> (Id, &'db T) {
        // SAFETY: `ZalsaLocal` is `!Sync`, and we never expose a reference to this field,
        // so we have exclusive access.
        let most_recent_pages = unsafe { &mut *self.most_recent_pages.get() };

        // Fast-path, we already have an unfilled page available.
        if let Some(&page) = most_recent_pages.get(&ingredient) {
            let page_ref = zalsa.table().page::<T>(page);

            // SAFETY: `ZalsaLocal` is `!Sync`, and we only insert a page into `most_recent_pages`
            // if it was allocated by our thread, so we are the unique writer.
            match unsafe { page_ref.allocate(page, value) } {
                Ok((id, value)) => return (id, value),
                Err(v) => value = v,
            }
        }

        self.allocate_cold(zalsa, ingredient, value)
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn allocate_cold<'db, T: Slot>(
        &self,
        zalsa: &'db Zalsa,
        ingredient: IngredientIndex,
        mut value: impl FnOnce(Id) -> T,
    ) -> (Id, &'db T) {
        let memo_types = || {
            zalsa
                .lookup_ingredient(ingredient)
                .memo_table_types()
                .clone()
        };

        // SAFETY: `ZalsaLocal` is `!Sync`, and we never expose a reference to this field,
        // so we have exclusive access.
        let most_recent_pages = unsafe { &mut *self.most_recent_pages.get() };

        // Find the most recent page, pushing a page if needed
        let mut page = *most_recent_pages.entry(ingredient).or_insert_with(|| {
            zalsa
                .table()
                .fetch_or_push_page::<T>(ingredient, memo_types)
        });

        loop {
            // Try to allocate an entry on that page
            let page_ref = zalsa.table().page::<T>(page);

            // SAFETY: `ZalsaLocal` is `!Sync`, and we only insert a page into `most_recent_pages`
            // if it was allocated by our thread, so we are the unique writer.
            match unsafe { page_ref.allocate(page, value) } {
                // If successful, return
                Ok((id, value)) => return (id, value),

                // Otherwise, create a new page and try again.
                //
                // Note that we could try fetching a page again, but as we just filled one up
                // it is unlikely that there is a non-full one available.
                Err(v) => {
                    value = v;
                    page = zalsa.table().push_page::<T>(ingredient, memo_types());
                    most_recent_pages.insert(ingredient, page);
                }
            }
        }
    }

    #[inline]
    pub(crate) fn push_query(
        &self,
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) -> ActiveQueryGuard<'_> {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                stack.push_new_query(database_key_index, iteration_count);

                ActiveQueryGuard {
                    local_state: self,
                    database_key_index,
                    #[cfg(debug_assertions)]
                    push_len: stack.len(),
                }
            })
        }
    }

    /// Executes a closure within the context of the current active query stacks (mutable).
    ///
    /// # Safety
    ///
    /// The closure cannot access the query stack reentrantly, whether mutable or immutable.
    #[inline(always)]
    pub(crate) unsafe fn with_query_stack_unchecked_mut<R>(
        &self,
        f: impl UnwindSafe + FnOnce(&mut QueryStack) -> R,
    ) -> R {
        // SAFETY: The caller guarantees that the query stack will not be accessed reentrantly.
        // Additionally, `ZalsaLocal` is `!Sync`, and we never expose a reference to the query
        // stack except through this method, so we have exclusive access.
        unsafe { f(&mut self.query_stack.try_borrow_mut().unwrap_unchecked()) }
    }

    /// Executes a closure within the context of the current active query stacks.
    ///
    /// # Safety
    ///
    /// No mutable references to the query stack can exist while the closure is executed.
    #[inline(always)]
    pub(crate) unsafe fn with_query_stack_unchecked<R>(
        &self,
        f: impl UnwindSafe + FnOnce(&QueryStack) -> R,
    ) -> R {
        // SAFETY: The caller guarantees that the query stack will not being accessed mutably.
        // Additionally, `ZalsaLocal` is `!Sync`, and we never expose a reference to the query
        // stack except through this method, so we have exclusive access.
        unsafe { f(&self.query_stack.try_borrow().unwrap_unchecked()) }
    }

    #[inline(always)]
    pub(crate) fn try_with_query_stack<R>(
        &self,
        f: impl UnwindSafe + FnOnce(&QueryStack) -> R,
    ) -> Option<R> {
        self.query_stack
            .try_borrow()
            .ok()
            .as_ref()
            .map(|stack| f(stack))
    }

    /// Returns the index of the active query along with its *current* durability/changed-at
    /// information. As the query continues to execute, naturally, that information may change.
    pub(crate) fn active_query(&self) -> Option<(DatabaseKeyIndex, Stamp)> {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked(|stack| {
                stack
                    .last()
                    .map(|active_query| (active_query.database_key_index, active_query.stamp()))
            })
        }
    }

    /// Add an output to the current query's list of dependencies
    ///
    /// Returns `Err` if not in a query.
    #[cfg(feature = "accumulator")]
    pub(crate) fn accumulate<A: Accumulator>(
        &self,
        index: IngredientIndex,
        value: A,
    ) -> Result<(), ()> {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                if let Some(top_query) = stack.last_mut() {
                    top_query.accumulate(index, value);
                    Ok(())
                } else {
                    Err(())
                }
            })
        }
    }

    /// Add an output to the current query's list of dependencies
    pub(crate) fn add_output(&self, entity: DatabaseKeyIndex) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                if let Some(top_query) = stack.last_mut() {
                    top_query.add_output(entity)
                }
            })
        }
    }

    /// Check whether `entity` is a tracked struct that was created by the currently active query (if any)
    pub(crate) fn is_tracked_struct_of_active_query(&self, entity: DatabaseKeyIndex) -> bool {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                stack
                    .last_mut()
                    .is_some_and(|top_query| top_query.tracked_struct_ids().is_active(entity))
            })
        }
    }

    /// Register that currently active query reads the given input
    #[inline(always)]
    pub(crate) fn report_tracked_read(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
        cycle_heads: &CycleHeads,
        #[cfg(feature = "accumulator")] has_accumulated: bool,
        #[cfg(feature = "accumulator")] accumulated_inputs: &AtomicInputAccumulatedValues,
    ) {
        crate::tracing::debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input,
            durability,
            changed_at
        );

        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                if let Some(top_query) = stack.last_mut() {
                    top_query.add_read(
                        input,
                        durability,
                        changed_at,
                        cycle_heads,
                        #[cfg(feature = "accumulator")]
                        has_accumulated,
                        #[cfg(feature = "accumulator")]
                        accumulated_inputs,
                    );
                }
            })
        }
    }

    /// Register that currently active query reads the given input
    #[inline(always)]
    pub(crate) fn report_tracked_read_simple(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
    ) {
        crate::tracing::debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input,
            durability,
            changed_at
        );

        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                if let Some(top_query) = stack.last_mut() {
                    top_query.add_read_simple(input, durability, changed_at);
                }
            })
        }
    }

    /// Register that the current query read an untracked value
    ///
    /// # Parameters
    ///
    /// * `current_revision`, the current revision
    #[inline(always)]
    pub(crate) fn report_untracked_read(&self, current_revision: Revision) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                if let Some(top_query) = stack.last_mut() {
                    top_query.add_untracked_read(current_revision);
                }
            })
        }
    }

    /// Called when the active queries creates an index from the
    /// entity table with the index `entity_index`. Has the following effects:
    ///
    /// * Add a query read on `DatabaseKeyIndex::for_table(entity_index)`
    /// * Identify a unique disambiguator for the hash within the current query,
    ///   adding the hash to the current query's disambiguator table.
    /// * Returns a tuple of:
    ///   * the id of the current query
    ///   * the current dependencies (durability, changed_at) of current query
    ///   * the disambiguator index
    #[track_caller]
    pub(crate) fn disambiguate(&self, key: IdentityHash) -> (Stamp, Disambiguator) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                let top_query = stack.last_mut().expect(
                    "cannot create a tracked struct disambiguator outside of a tracked function",
                );
                let disambiguator = top_query.disambiguate(key);
                (top_query.stamp(), disambiguator)
            })
        }
    }

    #[track_caller]
    pub(crate) fn tracked_struct_id(&self, identity: &Identity) -> Option<Id> {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                let top_query = stack
                    .last_mut()
                    .expect("cannot create a tracked struct ID outside of a tracked function");
                top_query.tracked_struct_ids_mut().reuse(identity)
            })
        }
    }

    #[track_caller]
    pub(crate) fn store_tracked_struct_id(&self, identity: Identity, id: Id) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.with_query_stack_unchecked_mut(|stack| {
                let top_query = stack
                    .last_mut()
                    .expect("cannot store a tracked struct ID outside of a tracked function");
                top_query.tracked_struct_ids_mut().insert(identity, id);
            })
        }
    }

    #[inline]
    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.cancelled.clone()
    }

    #[inline]
    pub(crate) fn uncancel(&self) {
        self.cancelled.reset();
    }

    #[inline]
    pub fn should_trigger_local_cancellation(&self) -> bool {
        self.cancelled.should_trigger_local_cancellation()
    }

    #[cold]
    pub(crate) fn unwind_pending_write(&self) {
        Cancelled::PendingWrite.throw();
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self) {
        Cancelled::Local.throw();
    }

    #[inline]
    pub(crate) fn set_cancellation_disabled(&self, was_disabled: bool) -> bool {
        self.cancelled.set_cancellation_disabled(was_disabled)
    }
}

// Okay to implement as `ZalsaLocal`` is !Sync
// - `most_recent_pages` can't observe broken states as we cannot panic such that we enter an
//   inconsistent state
// - neither can `query_stack` as we require the closures accessing it to be `UnwindSafe`
impl std::panic::RefUnwindSafe for ZalsaLocal {}

/// Summarizes "all the inputs that a query used" and "all the outputs it has written to".
#[derive(Debug)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
// #[derive(Clone)] cloning this is expensive, so we don't derive
pub(crate) struct QueryRevisions {
    /// The most revision in which some input changed.
    pub(crate) changed_at: Revision,

    /// Minimum durability of the inputs to this query.
    pub(crate) durability: Durability,

    /// How was this query computed?
    pub(crate) origin: QueryOrigin,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any direct or indirect accumulated values.
    ///
    /// Note that this field could be in `QueryRevisionsExtra` as it is only relevant
    /// for accumulators, but we get it for free anyways due to padding.
    #[cfg(feature = "accumulator")]
    #[cfg_attr(feature = "persistence", serde(skip))] // TODO: Support serializing accumulators
    pub(super) accumulated_inputs: AtomicInputAccumulatedValues,

    /// Are the `cycle_heads` verified to not be provisional anymore?
    ///
    /// Note that this field could be in `QueryRevisionsExtra` as it is only
    /// relevant for queries that participate in a cycle, but we get it for
    /// free anyways due to padding.
    #[cfg_attr(feature = "persistence", serde(with = "persistence::verified_final"))]
    pub(super) verified_final: AtomicBool,

    /// Lazily allocated state.
    pub(super) extra: QueryRevisionsExtra,
}

impl QueryRevisions {
    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn allocation_size(&self) -> usize {
        let QueryRevisions {
            changed_at: _,
            durability: _,
            verified_final: _,
            origin,
            extra,
            #[cfg(feature = "accumulator")]
                accumulated_inputs: _,
        } = self;

        let mut memory = 0;

        if let QueryOriginRef::Derived(query_edges)
        | QueryOriginRef::DerivedUntracked(query_edges) = origin.as_ref()
        {
            memory += query_edges.allocation_size();
        }

        if let Some(extra) = extra.0.as_ref() {
            memory += std::mem::size_of::<QueryRevisionsExtra>();
            memory += extra.allocation_size();
        }

        memory
    }
}

/// Data on `QueryRevisions` that is lazily allocated to save memory
/// in the common case.
///
/// In particular, not all queries create tracked structs, participate
/// in cycles, or create accumulators.
#[derive(Debug, Default)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "persistence", serde(transparent))]
pub(crate) struct QueryRevisionsExtra(Option<Box<QueryRevisionsExtraInner>>);

impl QueryRevisionsExtra {
    pub fn new(
        #[cfg(feature = "accumulator")] accumulated: AccumulatedMap,
        mut tracked_struct_ids: ThinVec<(Identity, Id)>,
        cycle_heads: CycleHeads,
        iteration: IterationCount,
    ) -> Self {
        #[cfg(feature = "accumulator")]
        let acc = accumulated.is_empty();
        #[cfg(not(feature = "accumulator"))]
        let acc = true;
        let inner = if acc
            && tracked_struct_ids.is_empty()
            && cycle_heads.is_empty()
            && iteration.is_initial()
        {
            None
        } else {
            tracked_struct_ids.shrink_to_fit();

            Some(Box::new(QueryRevisionsExtraInner {
                #[cfg(feature = "accumulator")]
                accumulated,
                cycle_heads,
                tracked_struct_ids,
                iteration: iteration.into(),
                cycle_converged: false,
            }))
        };

        Self(inner)
    }
}

#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
struct QueryRevisionsExtraInner {
    #[cfg(feature = "accumulator")]
    #[cfg_attr(feature = "persistence", serde(skip))] // TODO: Support serializing accumulators
    accumulated: AccumulatedMap,

    /// The ids of tracked structs created by this query.
    ///
    /// This table plays an important role when queries are
    /// re-executed:
    /// * A clone of this field is used as the initial set of
    ///   `TrackedStructId`s for the query on the next execution.
    /// * The query will thus re-use the same ids if it creates
    ///   tracked structs with the same `KeyStruct` as before.
    ///   It may also create new tracked structs.
    /// * One tricky case involves deleted structs. If
    ///   the old revision created a struct S but the new
    ///   revision did not, there will still be a map entry
    ///   for S. This is because queries only ever grow the map
    ///   and they start with the same entries as from the
    ///   previous revision. To handle this, `diff_outputs` compares
    ///   the structs from the old/new revision and retains
    ///   only entries that appeared in the new revision.
    //
    // TODO: We only need to serialize the IDs of tracked structs that
    // are actually going to be serialized. Those that are not will
    // be created with new IDs anyways.
    tracked_struct_ids: ThinVec<(Identity, Id)>,

    /// This result was computed based on provisional values from
    /// these cycle heads. The "cycle head" is the query responsible
    /// for managing a fixpoint iteration. In a cycle like
    /// `--> A --> B --> C --> A`, the cycle head is query `A`: it is
    /// the query whose value is requested while it is executing,
    /// which must provide the initial provisional value and decide,
    /// after each iteration, whether the cycle has converged or must
    /// iterate again.
    cycle_heads: CycleHeads,

    iteration: AtomicIterationCount,

    /// Stores for nested cycle heads whether they've converged in the last iteration.
    /// This value is always `false` for other queries.
    #[cfg_attr(feature = "persistence", serde(skip))]
    cycle_converged: bool,
}

impl QueryRevisionsExtraInner {
    #[cfg(feature = "salsa_unstable")]
    fn allocation_size(&self) -> usize {
        let QueryRevisionsExtraInner {
            #[cfg(feature = "accumulator")]
            accumulated,
            tracked_struct_ids,
            cycle_heads,
            iteration: _,
            cycle_converged: _,
        } = self;

        #[cfg(feature = "accumulator")]
        let b = accumulated.allocation_size();
        #[cfg(not(feature = "accumulator"))]
        let b = 0;
        b + cycle_heads.allocation_size() + std::mem::size_of_val(tracked_struct_ids.as_slice())
    }
}

impl fmt::Debug for QueryRevisionsExtraInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        struct FmtTrackedStructIds<'a>(&'a ThinVec<(Identity, Id)>);

        impl fmt::Debug for FmtTrackedStructIds<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                let mut f = f.debug_list();

                if self.0.len() > 5 {
                    f.entries(&self.0[..5]);
                    f.finish_non_exhaustive()
                } else {
                    f.entries(self.0);
                    f.finish()
                }
            }
        }

        let mut f = f.debug_struct("QueryRevisionsExtraInner");

        f.field("cycle_heads", &self.cycle_heads)
            .field("iteration", &self.iteration)
            .field("cycle_converged", &self.cycle_converged);

        #[cfg(feature = "accumulator")]
        {
            f.field("accumulated", &self.accumulated);
        }

        f.field(
            "tracked_struct_ids",
            &FmtTrackedStructIds(&self.tracked_struct_ids),
        );

        f.finish()
    }
}

#[cfg(not(feature = "shuttle"))]
#[cfg(target_pointer_width = "64")]
const _: [(); std::mem::size_of::<QueryRevisions>()] = [(); std::mem::size_of::<[usize; 4]>()];

#[cfg(not(feature = "shuttle"))]
#[cfg(target_pointer_width = "64")]
const _: [(); std::mem::size_of::<QueryRevisionsExtraInner>()] =
    [(); std::mem::size_of::<[usize; if cfg!(feature = "accumulator") { 7 } else { 3 }]>()];

impl QueryRevisions {
    pub(crate) fn fixpoint_initial(query: DatabaseKeyIndex, iteration: IterationCount) -> Self {
        Self {
            changed_at: Revision::start(),
            durability: Durability::MAX,
            origin: QueryOrigin::derived([]),
            #[cfg(feature = "accumulator")]
            accumulated_inputs: Default::default(),
            verified_final: AtomicBool::new(false),
            extra: QueryRevisionsExtra::new(
                #[cfg(feature = "accumulator")]
                AccumulatedMap::default(),
                ThinVec::default(),
                CycleHeads::initial(query, iteration),
                iteration,
            ),
        }
    }

    /// Returns a reference to the `AccumulatedMap` for this query, or `None` if the map is empty.
    #[cfg(feature = "accumulator")]
    pub(crate) fn accumulated(&self) -> Option<&AccumulatedMap> {
        self.extra
            .0
            .as_deref()
            .map(|extra| &extra.accumulated)
            .filter(|map| !map.is_empty())
    }

    /// Returns a reference to the `CycleHeads` for this query.
    pub(crate) fn cycle_heads(&self) -> &CycleHeads {
        match &self.extra.0 {
            Some(extra) => &extra.cycle_heads,
            None => empty_cycle_heads(),
        }
    }

    /// Sets the `CycleHeads` for this query.
    pub(crate) fn set_cycle_heads(&mut self, cycle_heads: CycleHeads) {
        match &mut self.extra.0 {
            Some(extra) => extra.cycle_heads = cycle_heads,
            None => {
                self.extra = QueryRevisionsExtra::new(
                    #[cfg(feature = "accumulator")]
                    AccumulatedMap::default(),
                    ThinVec::default(),
                    cycle_heads,
                    IterationCount::default(),
                );
            }
        };
    }

    pub(crate) fn cycle_converged(&self) -> bool {
        match &self.extra.0 {
            Some(extra) => extra.cycle_converged,
            None => false,
        }
    }

    pub(crate) fn set_cycle_converged(&mut self, cycle_converged: bool) {
        if let Some(extra) = &mut self.extra.0 {
            extra.cycle_converged = cycle_converged
        }
    }

    pub(crate) fn iteration(&self) -> IterationCount {
        match &self.extra.0 {
            Some(extra) => extra.iteration.load(),
            None => IterationCount::initial(),
        }
    }

    pub(crate) fn set_iteration_count(
        &self,
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) {
        let Some(extra) = &self.extra.0 else {
            return;
        };
        debug_assert!(extra.iteration.load() <= iteration_count);

        extra.iteration.store(iteration_count);

        extra
            .cycle_heads
            .update_iteration_count(database_key_index, iteration_count);
    }

    fn get_or_insert_extra(&mut self) -> &mut QueryRevisionsExtraInner {
        self.extra.0.get_or_insert_with(|| {
            Box::new(QueryRevisionsExtraInner {
                #[cfg(feature = "accumulator")]
                accumulated: AccumulatedMap::default(),
                tracked_struct_ids: ThinVec::default(),
                cycle_heads: empty_cycle_heads().clone(),
                iteration: IterationCount::default().into(),
                cycle_converged: false,
            })
        })
    }

    fn extra(&self) -> Option<&QueryRevisionsExtraInner> {
        self.extra.0.as_deref()
    }

    /// Updates the iteration count of the memo without updating the iteration in `cycle_heads`.
    ///
    /// Don't call this method on a cycle head, as it results in diverging iteration counts
    /// between what's in cycle heads and stored on the memo.
    pub(crate) fn update_cycle_participant_iteration_count(
        &mut self,
        iteration_count: IterationCount,
    ) {
        let extra = self.get_or_insert_extra();
        extra.iteration.set(iteration_count);
    }

    /// Updates the iteration count if this query has any cycle heads. Otherwise it's a no-op.
    pub(crate) fn update_iteration_count_mut(
        &mut self,
        cycle_head_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) {
        let extra = self.get_or_insert_extra();
        extra.iteration.set(iteration_count);
        extra
            .cycle_heads
            .update_iteration_count_mut(cycle_head_index, iteration_count);
    }

    /// Returns the ids of the tracked structs created when running this query.
    pub fn tracked_struct_ids(&self) -> &[(Identity, Id)] {
        self.extra()
            .map(|extra| &*extra.tracked_struct_ids)
            .unwrap_or_default()
    }

    /// Returns a mutable reference to the `IdentityMap` for this query, or `None` if the map is empty.
    pub fn tracked_struct_ids_mut(&mut self) -> Option<&mut ThinVec<(Identity, Id)>> {
        self.extra
            .0
            .as_mut()
            .map(|extra| &mut extra.tracked_struct_ids)
            .filter(|tracked_struct_ids| !tracked_struct_ids.is_empty())
    }
}

/// Tracks the way that a memoized value for a query was created.
///
/// This is a read-only reference to a `PackedQueryOrigin`.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize))]
#[cfg_attr(feature = "persistence", serde(rename = "QueryOrigin"))]
pub enum QueryOriginRef<'a> {
    /// The value was assigned as the output of another query (e.g., using `specify`).
    /// The `DatabaseKeyIndex` is the identity of the assigning query.
    Assigned(DatabaseKeyIndex) = QueryOriginKind::Assigned as u8,

    /// The value was derived by executing a function
    /// and we were able to track ALL of that function's inputs.
    /// Those inputs are described in [`QueryEdges`].
    Derived(QueryEdges<'a>) = QueryOriginKind::Derived as u8,

    /// The value was derived by executing a function
    /// but that function also reported that it read untracked inputs.
    /// The [`QueryEdges`] argument contains a listing of all the inputs we saw
    /// (but we know there were more).
    DerivedUntracked(QueryEdges<'a>) = QueryOriginKind::DerivedUntracked as u8,
}

impl<'a> QueryOriginRef<'a> {
    /// Indices for queries *read* by this query
    #[inline]
    pub(crate) fn inputs(self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + use<'a> {
        self.edges().iter().filter_map(|edge| match edge.kind() {
            QueryEdgeKind::Input => Some(edge.key()),
            QueryEdgeKind::Output => None,
        })
    }

    /// Indices for queries *written* by this query (if any)
    pub(crate) fn outputs(self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + use<'a> {
        let opt_edges = match self {
            QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => Some(edges),
            QueryOriginRef::Assigned(_) => None,
        };
        opt_edges.into_iter().flat_map(output_edges)
    }

    #[inline]
    pub(crate) fn edges(self) -> QueryEdges<'a> {
        match self {
            QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => edges,
            QueryOriginRef::Assigned(_) => QueryEdges::wide(&[]),
        }
    }
}

// Note: The discriminant assignment is intentional,
// we want to group `Derived` and `DerivedUntracked` together on a same bit (the second LSB)
// as we tend to match against both of them in the same branch.
#[derive(Clone, Copy)]
#[repr(u8)]
enum QueryOriginKind {
    /// The value was assigned as the output of another query.
    ///
    /// This can, for example, can occur when `specify` is used.
    Assigned = 0b01,

    /// The value was derived by executing a function
    /// _and_ Salsa was able to track all of said function's inputs.
    Derived = 0b11,

    /// The value was derived by executing a function
    /// but that function also reported that it read untracked inputs.
    DerivedUntracked = 0b10,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum QueryEdgeLayout {
    /// Every edge in the origin fits in [`PackedQueryEdge`].
    ///
    /// The origin stores one `Box<[PackedQueryEdge]>`, reducing each retained edge
    /// from 12 bytes to 8 bytes.
    Packed = 0b000,

    /// At least one edge in the origin does not fit in [`PackedQueryEdge`].
    ///
    /// The origin retains the original `Box<[QueryEdge]>`. Spilling the entire origin
    /// avoids allocating a separate overflow object for each wide edge.
    Wide = 0b100,
}

/// Encodes the semantic origin kind and retained edge layout in a single byte.
#[derive(Clone, Copy)]
#[repr(transparent)]
struct QueryOriginTag(u8);

impl QueryOriginTag {
    const KIND_MASK: u8 = 0b011;
    const LAYOUT_MASK: u8 = 0b100;

    const fn new(kind: QueryOriginKind, layout: QueryEdgeLayout) -> Self {
        debug_assert!(kind as u8 & layout as u8 == 0);
        QueryOriginTag(kind as u8 | layout as u8)
    }

    const fn kind(self) -> QueryOriginKind {
        match self.0 & Self::KIND_MASK {
            0b01 => QueryOriginKind::Assigned,
            0b11 => QueryOriginKind::Derived,
            0b10 => QueryOriginKind::DerivedUntracked,
            _ => panic!("invalid query origin kind"),
        }
    }

    const fn layout(self) -> QueryEdgeLayout {
        if self.0 & Self::LAYOUT_MASK == 0 {
            QueryEdgeLayout::Packed
        } else {
            QueryEdgeLayout::Wide
        }
    }
}

/// Tracks how a memoized value for a given query was created.
///
/// Derived origins retain their edges in a single allocation. If every edge fits in
/// [`PackedQueryEdge`], the origin stores a `Box<[PackedQueryEdge]>`. Otherwise, it keeps
/// the original `Box<[QueryEdge]>`, avoiding separate allocations for overflowing edges.
///
/// [`QueryOriginTag`] stores the semantic origin kind independently from the retained
/// [`QueryEdgeLayout`]. Consumers see the same assigned, derived, and untracked-derived
/// origins regardless of how their edges are stored.
///
/// This type is a manual enum packed to 13 bytes to reduce the size of `QueryRevisions`.
#[repr(Rust, packed)]
pub struct QueryOrigin {
    /// The tag of this enum.
    ///
    /// Note that this tag only requires three bits and could likely be packed into
    /// some other field. However, we get this byte for free thanks to alignment.
    tag: QueryOriginTag,

    /// The data portion of this enum.
    data: QueryOriginData,

    /// The metadata of this enum.
    ///
    /// For the derived variants, this is the length of the `input_outputs` allocation.
    ///
    /// For `QueryOriginKind::Assigned`, this is the `IngredientIndex` of assigning query.
    /// Combined with the `Id` data, this forms a complete `DatabaseKeyIndex`.
    metadata: u32,
}

/// The data portion of `PackedQueryOrigin`.
union QueryOriginData {
    /// Query edges for the derived variants.
    ///
    /// The query edges are between a memoized value and other queries in the dependency graph,
    /// including both dependency edges (e.g., when creating the memoized value for Q0
    /// executed another function Q1) and output edges (e.g., when Q0 specified the value
    /// for another query Q2).
    ///
    /// Note that we always track input dependencies even when there are untracked reads.
    /// Untracked reads mean that Salsa can't verify values, so the list of inputs is unused.
    /// However, Salsa still uses these edges to find the transitive inputs to an accumulator.
    ///
    /// You can access the input/output list via the methods [`inputs`] and [`outputs`] respectively.
    ///
    /// Important:
    ///
    /// * The inputs must be in **execution order** for the red-green algorithm to work.
    input_outputs: NonNull<()>,

    /// The identity of the assigning query for `QueryOriginKind::Assigned`.
    index: Id,
}

/// SAFETY: The `input_outputs` pointer is owned and not accessed or shared concurrently.
unsafe impl Send for QueryOriginData {}
/// SAFETY: Same as above.
unsafe impl Sync for QueryOriginData {}

impl QueryOrigin {
    pub const fn is_derived_untracked(&self) -> bool {
        matches!(self.tag.kind(), QueryOriginKind::DerivedUntracked)
    }

    /// Create a query origin of type `QueryOriginKind::Derived`, with the given edges.
    pub fn derived<I>(input_outputs: I) -> QueryOrigin
    where
        I: IntoIterator<Item = QueryEdge>,
        I::IntoIter: ExactSizeIterator,
    {
        Self::derived_with_kind(input_outputs.into_iter(), QueryOriginKind::Derived)
    }

    fn derived_with_kind(
        mut input_outputs: impl ExactSizeIterator<Item = QueryEdge>,
        kind: QueryOriginKind,
    ) -> QueryOrigin {
        // Exceeding `u32::MAX` query edges should never happen in real-world usage.
        let length = u32::try_from(input_outputs.len())
            .expect("exceeded more than `u32::MAX` query edges; this should never happen.");

        let mut packed_input_outputs = Vec::with_capacity(length as usize);

        let (layout, input_outputs) = 'edges: {
            for edge in input_outputs.by_ref() {
                let Some(edge) = PackedQueryEdge::new(edge) else {
                    let mut wide_input_outputs = Vec::with_capacity(length as usize);
                    wide_input_outputs
                        .extend(packed_input_outputs.into_iter().map(PackedQueryEdge::edge));
                    wide_input_outputs.push(edge);
                    wide_input_outputs.extend(input_outputs);
                    let input_outputs = wide_input_outputs.into_boxed_slice();

                    // SAFETY: `Box::into_raw` returns a non-null pointer.
                    let input_outputs = unsafe {
                        NonNull::new_unchecked(Box::into_raw(input_outputs).cast::<QueryEdge>())
                            .cast::<()>()
                    };

                    break 'edges (QueryEdgeLayout::Wide, input_outputs);
                };

                packed_input_outputs.push(edge);
            }

            let input_outputs = packed_input_outputs.into_boxed_slice();

            // SAFETY: `Box::into_raw` returns a non-null pointer.
            let input_outputs = unsafe {
                NonNull::new_unchecked(Box::into_raw(input_outputs).cast::<PackedQueryEdge>())
                    .cast::<()>()
            };

            (QueryEdgeLayout::Packed, input_outputs)
        };

        QueryOrigin {
            tag: QueryOriginTag::new(kind, layout),
            metadata: length,
            data: QueryOriginData { input_outputs },
        }
    }

    /// Create a query origin of type `QueryOriginKind::DerivedUntracked`, with the given edges.
    pub fn derived_untracked<I>(input_outputs: I) -> QueryOrigin
    where
        I: IntoIterator<Item = QueryEdge>,
        I::IntoIter: ExactSizeIterator,
    {
        Self::derived_with_kind(input_outputs.into_iter(), QueryOriginKind::DerivedUntracked)
    }

    /// Sets the `input_outputs` of this query's origin if it's derived or derived untracked.
    /// Returns `Err` if the query origin isn't derived.
    pub(crate) fn set_edges<I>(&mut self, input_outputs: I) -> Result<(), I>
    where
        I: IntoIterator<Item = QueryEdge>,
        I::IntoIter: ExactSizeIterator,
    {
        match self.tag.kind() {
            QueryOriginKind::Assigned => Err(input_outputs),
            QueryOriginKind::Derived => {
                *self = QueryOrigin::derived(input_outputs);
                Ok(())
            }
            QueryOriginKind::DerivedUntracked => {
                *self = QueryOrigin::derived_untracked(input_outputs);
                Ok(())
            }
        }
    }

    /// Create a query origin of type `QueryOriginKind::Assigned`, with the given key.
    pub fn assigned(key: DatabaseKeyIndex) -> QueryOrigin {
        QueryOrigin {
            tag: QueryOriginTag::new(QueryOriginKind::Assigned, QueryEdgeLayout::Packed),
            metadata: key.ingredient_index().as_u32(),
            data: QueryOriginData {
                index: key.key_index(),
            },
        }
    }

    /// Return a read-only reference to this query origin.
    pub fn as_ref(&self) -> QueryOriginRef<'_> {
        match self.tag.kind() {
            QueryOriginKind::Assigned => {
                // SAFETY: `data.index` is initialized when the tag is `QueryOriginKind::Assigned`.
                let index = unsafe { self.data.index };

                // SAFETY: `metadata` is initialized from a valid `IngredientIndex` when the tag
                // is `QueryOriginKind::Assigned`.
                let ingredient_index = unsafe { IngredientIndex::new_unchecked(self.metadata) };

                QueryOriginRef::Assigned(DatabaseKeyIndex::new(ingredient_index, index))
            }

            QueryOriginKind::Derived => {
                // SAFETY: Derived origins initialize `data.input_outputs` with a boxed slice
                // matching `tag.layout()`, and `metadata` stores that slice's length.
                QueryOriginRef::Derived(unsafe {
                    QueryEdges::from_raw(
                        self.tag.layout(),
                        self.data.input_outputs,
                        self.metadata as usize,
                    )
                })
            }

            QueryOriginKind::DerivedUntracked => {
                // SAFETY: Untracked derived origins initialize `data.input_outputs` with a boxed
                // slice matching `tag.layout()`, and `metadata` stores that slice's length.
                QueryOriginRef::DerivedUntracked(unsafe {
                    QueryEdges::from_raw(
                        self.tag.layout(),
                        self.data.input_outputs,
                        self.metadata as usize,
                    )
                })
            }
        }
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for QueryOrigin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_ref().serialize(serializer)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for QueryOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Matches the signature of `QueryOriginRef`.
        #[repr(u8)]
        #[derive(serde::Deserialize)]
        #[serde(rename = "QueryOrigin")]
        pub enum QueryOriginOwned {
            Assigned(DatabaseKeyIndex) = QueryOriginKind::Assigned as u8,
            Derived(Box<[QueryEdge]>) = QueryOriginKind::Derived as u8,
            DerivedUntracked(Box<[QueryEdge]>) = QueryOriginKind::DerivedUntracked as u8,
        }

        Ok(match QueryOriginOwned::deserialize(deserializer)? {
            QueryOriginOwned::Assigned(key) => QueryOrigin::assigned(key),
            QueryOriginOwned::Derived(edges) => QueryOrigin::derived(edges),
            QueryOriginOwned::DerivedUntracked(edges) => QueryOrigin::derived_untracked(edges),
        })
    }
}

impl Drop for QueryOrigin {
    fn drop(&mut self) {
        match self.tag.kind() {
            QueryOriginKind::Derived | QueryOriginKind::DerivedUntracked => {
                // SAFETY: Derived origin kinds initialize `data.input_outputs`.
                let input_outputs = unsafe { self.data.input_outputs };
                let length = self.metadata as usize;

                match self.tag.layout() {
                    QueryEdgeLayout::Packed => {
                        // SAFETY: Packed layouts store a boxed slice of `PackedQueryEdge`.
                        drop(unsafe {
                            Box::from_raw(ptr::slice_from_raw_parts_mut(
                                input_outputs.cast::<PackedQueryEdge>().as_ptr(),
                                length,
                            ))
                        });
                    }
                    QueryEdgeLayout::Wide => {
                        // SAFETY: Wide layouts store a boxed slice of `QueryEdge`.
                        drop(unsafe {
                            Box::from_raw(ptr::slice_from_raw_parts_mut(
                                input_outputs.cast::<QueryEdge>().as_ptr(),
                                length,
                            ))
                        });
                    }
                }
            }

            // The data stored for this variant is `Copy`.
            QueryOriginKind::Assigned => {}
        }
    }
}

impl std::fmt::Debug for QueryOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

/// An input or output query edge.
///
/// This type stores the [`QueryEdgeKind`] as a tag on the `IngredientIndex` without
/// increasing the size of the type. Its 12-byte size is meaningful because inputs and
/// outputs are stored contiguously.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct QueryEdge {
    // Store a normalized zero-based index rather than nesting an `Id`, whose index uses a
    // `NonZeroU32(index + 1)` representation. Packed origins unpack many transient
    // `QueryEdge`s while flattening cycles; keeping the fields split lets that conversion,
    // `kind`, `Hash`, and `Eq` operate on the decoded words directly.
    index: u32,
    generation: u32,
    ingredient: IngredientIndex,
}

impl QueryEdge {
    /// Create an input query edge with the given index.
    pub const fn input(key: DatabaseKeyIndex) -> QueryEdge {
        let id = key.key_index();

        QueryEdge {
            index: id.index(),
            generation: id.generation(),
            ingredient: key.ingredient_index(),
        }
    }

    /// Create an output query edge with the given index.
    pub const fn output(key: DatabaseKeyIndex) -> QueryEdge {
        let mut edge = Self::input(key);
        edge.ingredient = edge.ingredient.with_tag(true);
        edge
    }

    /// Return the key of this query edge.
    pub const fn key(self) -> DatabaseKeyIndex {
        // Clear the tag to restore the original index.
        DatabaseKeyIndex::new(self.ingredient.with_tag(false), self.id())
    }

    /// Returns the kind of this query edge.
    pub const fn kind(self) -> QueryEdgeKind {
        if self.ingredient.tag() {
            QueryEdgeKind::Output
        } else {
            QueryEdgeKind::Input
        }
    }

    #[cfg(feature = "persistence")]
    const fn raw_key(self) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.ingredient, self.id())
    }

    const fn id(self) -> Id {
        // SAFETY: `index` came from a valid `Id` in `QueryEdge::input`.
        unsafe { Id::from_index(self.index) }.with_generation(self.generation)
    }
}

impl std::fmt::Debug for QueryEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut tuple = match self.kind() {
            QueryEdgeKind::Input => f.debug_tuple("Input"),
            QueryEdgeKind::Output => f.debug_tuple("Output"),
        };
        tuple.field(&self.key()).finish()
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for QueryEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.raw_key().serialize(serializer)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for QueryEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        DatabaseKeyIndex::deserialize(deserializer).map(QueryEdge::input)
    }
}

/// A retained 8-byte query edge.
///
/// Query origins use this representation when every edge fits. Otherwise, the origin
/// retains the existing full-width [`QueryEdge`] slice.
///
/// `metadata` has the following layout, from most significant to least significant
/// bit:
///
/// ```text
/// [ingredient: 12][generation: 20]
/// ```
///
/// `index` stores the complete 32-bit key index separately. Only input edges can use
/// this representation. Output edges, ingredient indices, and generations that do
/// not fit use the wide [`QueryEdge`] layout.
#[derive(Copy, Clone)]
struct PackedQueryEdge {
    index: u32,
    metadata: u32,
}

impl PackedQueryEdge {
    const INGREDIENT_SHIFT: u32 = 20;
    const GENERATION_MASK: u32 = 0xFFFFF;
    const INGREDIENT_MASK: u32 = 0xFFF;

    const fn new(edge: QueryEdge) -> Option<Self> {
        let ingredient = edge.ingredient.as_u32();

        if ingredient > Self::INGREDIENT_MASK || edge.generation > Self::GENERATION_MASK {
            return None;
        }

        Some(PackedQueryEdge {
            index: edge.index,
            metadata: edge.generation | (ingredient << Self::INGREDIENT_SHIFT),
        })
    }

    const fn edge(self) -> QueryEdge {
        QueryEdge {
            index: self.index,
            generation: self.metadata & Self::GENERATION_MASK,
            // SAFETY: `metadata` was built from an `IngredientIndex` in `PackedQueryEdge::new`.
            ingredient: unsafe {
                IngredientIndex::new_unchecked(self.metadata >> Self::INGREDIENT_SHIFT)
            },
        }
    }
}

/// A read-only view over the retained edges for a query origin.
#[derive(Copy, Clone)]
pub struct QueryEdges<'a> {
    data: QueryEdgesData<'a>,
}

#[derive(Copy, Clone)]
enum QueryEdgesData<'a> {
    Packed(&'a [PackedQueryEdge]),
    Wide(&'a [QueryEdge]),
}

impl<'a> QueryEdges<'a> {
    fn packed(edges: &'a [PackedQueryEdge]) -> Self {
        QueryEdges {
            data: QueryEdgesData::Packed(edges),
        }
    }

    fn wide(edges: &'a [QueryEdge]) -> Self {
        QueryEdges {
            data: QueryEdgesData::Wide(edges),
        }
    }

    /// # Safety
    ///
    /// `input_outputs` and `length` must form a valid slice of the type selected by `layout`
    /// for the lifetime `'a`.
    unsafe fn from_raw(layout: QueryEdgeLayout, input_outputs: NonNull<()>, length: usize) -> Self {
        match layout {
            QueryEdgeLayout::Packed => {
                // SAFETY: Caller obligation.
                QueryEdges::packed(unsafe {
                    std::slice::from_raw_parts(
                        input_outputs.cast::<PackedQueryEdge>().as_ptr(),
                        length,
                    )
                })
            }
            QueryEdgeLayout::Wide => {
                // SAFETY: Caller obligation.
                QueryEdges::wide(unsafe {
                    std::slice::from_raw_parts(input_outputs.cast::<QueryEdge>().as_ptr(), length)
                })
            }
        }
    }

    pub(crate) fn len(self) -> usize {
        match self.data {
            QueryEdgesData::Packed(edges) => edges.len(),
            QueryEdgesData::Wide(edges) => edges.len(),
        }
    }

    pub(crate) fn allocation_size(self) -> usize {
        match self.data {
            QueryEdgesData::Packed(edges) => std::mem::size_of_val(edges),
            QueryEdgesData::Wide(edges) => std::mem::size_of_val(edges),
        }
    }

    pub(crate) fn iter(self) -> QueryEdgeIter<'a> {
        let data = match self.data {
            QueryEdgesData::Packed(edges) => QueryEdgeIterData::Packed(edges.iter()),
            QueryEdgesData::Wide(edges) => QueryEdgeIterData::Wide(edges.iter()),
        };

        QueryEdgeIter { data }
    }

    pub(crate) fn iter_outputs(self) -> impl DoubleEndedIterator<Item = QueryEdge> + use<'a> {
        let wide_edges = match self.data {
            QueryEdgesData::Packed(_) => &[][..],
            QueryEdgesData::Wide(edges) => edges,
        };

        wide_edges
            .iter()
            .copied()
            .filter(|edge| matches!(edge.kind(), QueryEdgeKind::Output))
    }

    #[cfg(test)]
    fn is_packed(self) -> bool {
        matches!(self.data, QueryEdgesData::Packed(_))
    }
}

pub struct QueryEdgeIter<'a> {
    data: QueryEdgeIterData<'a>,
}

enum QueryEdgeIterData<'a> {
    Packed(std::slice::Iter<'a, PackedQueryEdge>),
    Wide(std::slice::Iter<'a, QueryEdge>),
}

impl Iterator for QueryEdgeIter<'_> {
    type Item = QueryEdge;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.data {
            QueryEdgeIterData::Packed(edges) => edges.next().copied().map(PackedQueryEdge::edge),
            QueryEdgeIterData::Wide(edges) => edges.next().copied(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
}

impl DoubleEndedIterator for QueryEdgeIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match &mut self.data {
            QueryEdgeIterData::Packed(edges) => {
                edges.next_back().copied().map(PackedQueryEdge::edge)
            }
            QueryEdgeIterData::Wide(edges) => edges.next_back().copied(),
        }
    }
}

impl ExactSizeIterator for QueryEdgeIter<'_> {
    fn len(&self) -> usize {
        match &self.data {
            QueryEdgeIterData::Packed(edges) => edges.len(),
            QueryEdgeIterData::Wide(edges) => edges.len(),
        }
    }
}

impl std::iter::FusedIterator for QueryEdgeIter<'_> {}

impl<'a> IntoIterator for QueryEdges<'a> {
    type Item = QueryEdge;
    type IntoIter = QueryEdgeIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl std::fmt::Debug for QueryEdges<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for QueryEdges<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QueryEdgeKind {
    Input,
    Output,
}

/// Returns the (tracked) outputs that were executed in computing this memoized value.
///
/// These will always be in execution order.
pub(crate) fn output_edges(
    input_outputs: QueryEdges<'_>,
) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + use<'_> {
    input_outputs.iter_outputs().map(QueryEdge::key)
}

/// When a query is pushed onto the `active_query` stack, this guard
/// is returned to represent its slot. The guard can be used to pop
/// the query from the stack -- in the case of unwinding, the guard's
/// destructor will also remove the query.
pub(crate) struct ActiveQueryGuard<'me> {
    local_state: &'me ZalsaLocal,
    #[cfg(debug_assertions)]
    push_len: usize,
    pub(crate) database_key_index: DatabaseKeyIndex,
}

impl ActiveQueryGuard<'_> {
    /// Initialize the tracked struct ids with the values from the prior execution.
    pub(crate) fn seed_tracked_struct_ids(&self, tracked_struct_ids: &[(Identity, Id)]) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.local_state.with_query_stack_unchecked_mut(|stack| {
                #[cfg(debug_assertions)]
                assert_eq!(stack.len(), self.push_len, "mismatched push and pop");
                let frame = stack.last_mut().unwrap();
                frame.tracked_struct_ids_mut().seed(tracked_struct_ids);
            })
        }
    }

    /// Append the given `outputs` to the query's output list.
    pub(crate) fn seed_iteration(&self, previous: &QueryRevisions) {
        let durability = previous.durability;
        let changed_at = previous.changed_at;
        let edges = previous.origin.as_ref().edges();
        let untracked_read = matches!(
            previous.origin.as_ref(),
            QueryOriginRef::DerivedUntracked(_)
        );

        let tracked_ids = previous.tracked_struct_ids();

        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.local_state.with_query_stack_unchecked_mut(|stack| {
                #[cfg(debug_assertions)]
                assert_eq!(stack.len(), self.push_len, "mismatched push and pop");
                let frame = stack.last_mut().unwrap();
                frame.seed_iteration(durability, changed_at, edges, untracked_read, tracked_ids);
            })
        }
    }

    pub(crate) fn take_cycle_heads(&mut self) -> CycleHeads {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.local_state.with_query_stack_unchecked_mut(|stack| {
                #[cfg(debug_assertions)]
                assert_eq!(stack.len(), self.push_len);
                let frame = stack.last_mut().unwrap();
                frame.take_cycle_heads()
            })
        }
    }

    /// Invoked when the query has successfully completed execution.
    fn complete(self) -> CompletedQuery {
        // SAFETY: We do not access the query stack reentrantly.
        let query = unsafe {
            self.local_state.with_query_stack_unchecked_mut(|stack| {
                stack.pop_into_revisions(
                    self.database_key_index,
                    #[cfg(debug_assertions)]
                    self.push_len,
                )
            })
        };
        std::mem::forget(self);
        query
    }

    /// Pops an active query from the stack. Returns the [`CompletedQuery`]
    /// which summarizes the other queries that were accessed during this
    /// query's execution.
    #[inline]
    pub(crate) fn pop(self) -> CompletedQuery {
        self.complete()
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            self.local_state.with_query_stack_unchecked_mut(|stack| {
                stack.pop(
                    self.database_key_index,
                    #[cfg(debug_assertions)]
                    self.push_len,
                );
            })
        };
    }
}

#[cfg(feature = "persistence")]
pub(crate) mod persistence {
    use super::{QueryOrigin, QueryRevisions, QueryRevisionsExtra};
    use crate::sync::atomic::{AtomicBool, Ordering};
    use crate::{Durability, Revision};

    /// A reference to the fields of [`QueryRevisions`], with its [`QueryOrigin`] transformed.
    #[derive(serde::Serialize)]
    pub(crate) struct MappedQueryRevisions<'a> {
        changed_at: Revision,
        durability: Durability,
        origin: QueryOrigin,
        #[serde(with = "verified_final")]
        verified_final: AtomicBool,
        extra: &'a QueryRevisionsExtra,
    }

    impl QueryRevisions {
        pub(crate) fn with_origin(&self, origin: QueryOrigin) -> MappedQueryRevisions<'_> {
            let QueryRevisions {
                changed_at,
                durability,
                ref verified_final,
                ref extra,
                #[cfg(feature = "accumulator")]
                    accumulated_inputs: _, // TODO: Support serializing accumulators
                origin: _,
            } = *self;

            MappedQueryRevisions {
                changed_at,
                durability,
                extra,
                origin,
                verified_final: AtomicBool::new(verified_final.load(Ordering::Relaxed)),
            }
        }
    }

    // A workaround the fact that `shuttle` atomic types do not implement `serde::{Serialize, Deserialize}`.
    pub(super) mod verified_final {
        use crate::sync::atomic::{AtomicBool, Ordering};

        pub fn serialize<S>(value: &AtomicBool, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serde::Serialize::serialize(&value.load(Ordering::Relaxed), serializer)
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<AtomicBool, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            serde::Deserialize::deserialize(deserializer).map(AtomicBool::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::{PackedQueryEdge, QueryEdge, QueryEdgeKind, QueryOrigin, QueryOriginRef};
    use crate::{DatabaseKeyIndex, Id, IngredientIndex};

    #[test]
    fn packed_query_edges_use_eight_bytes() {
        assert_eq!(size_of::<PackedQueryEdge>(), 8);
        assert_eq!(size_of::<QueryEdge>(), 12);
        assert_eq!(size_of::<QueryEdgeKind>(), 1);
        assert_eq!(size_of::<QueryOrigin>(), 13);
    }

    #[test]
    fn query_origin_packs_edges_that_fit() {
        let input = QueryEdge::input(key(231, 10_842_122, 41));
        let other_input = QueryEdge::input(key(232, 10_842_123, 42));
        let origin = QueryOrigin::derived([input, other_input]);
        let QueryOriginRef::Derived(edges) = origin.as_ref() else {
            panic!("expected derived origin");
        };

        assert!(edges.is_packed());
        assert_eq!(edges.allocation_size(), 2 * size_of::<PackedQueryEdge>());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![input, other_input]);
        assert_eq!(
            edges.iter().rev().collect::<Vec<_>>(),
            vec![other_input, input]
        );
        assert_eq!(
            origin.as_ref().inputs().collect::<Vec<_>>(),
            vec![input.key(), other_input.key()]
        );
        assert_eq!(origin.as_ref().outputs().collect::<Vec<_>>(), vec![]);
    }

    #[test]
    fn query_origin_spills_all_edges_if_it_contains_an_output() {
        let input = QueryEdge::input(key(231, 10_842_122, 41));
        let output = QueryEdge::output(key(232, 10_842_123, 42));
        let origin = QueryOrigin::derived([input, output]);
        let QueryOriginRef::Derived(edges) = origin.as_ref() else {
            panic!("expected derived origin");
        };

        assert!(!edges.is_packed());
        assert_eq!(edges.allocation_size(), 2 * size_of::<QueryEdge>());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![input, output]);
        assert_eq!(
            origin.as_ref().inputs().collect::<Vec<_>>(),
            vec![input.key()]
        );
        assert_eq!(
            origin.as_ref().outputs().collect::<Vec<_>>(),
            vec![output.key()]
        );
    }

    #[test]
    fn query_origin_packs_largest_supported_generation() {
        let input = QueryEdge::input(key(231, 10_842_122, PackedQueryEdge::GENERATION_MASK));
        let origin = QueryOrigin::derived([input]);
        let QueryOriginRef::Derived(edges) = origin.as_ref() else {
            panic!("expected derived origin");
        };

        assert!(edges.is_packed());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![input]);
    }

    #[test]
    fn query_origin_spills_all_edges_if_generation_does_not_fit() {
        let packed = QueryEdge::input(key(231, 10_842_122, 41));
        let wide = QueryEdge::output(key(232, 10_842_123, PackedQueryEdge::GENERATION_MASK + 1));
        let origin = QueryOrigin::derived([packed, wide]);
        let QueryOriginRef::Derived(edges) = origin.as_ref() else {
            panic!("expected derived origin");
        };

        assert!(!edges.is_packed());
        assert_eq!(edges.allocation_size(), 2 * size_of::<QueryEdge>());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![packed, wide]);
    }

    #[test]
    fn query_origin_spills_if_ingredient_does_not_fit() {
        let wide = QueryEdge::input(key(PackedQueryEdge::INGREDIENT_MASK + 1, 10_842_122, 41));
        let origin = QueryOrigin::derived([wide]);
        let QueryOriginRef::Derived(edges) = origin.as_ref() else {
            panic!("expected derived origin");
        };

        assert!(!edges.is_packed());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![wide]);
    }

    #[test]
    fn replacing_query_origin_edges_can_change_layout() {
        let packed = QueryEdge::input(key(231, 10_842_122, 41));
        let wide = QueryEdge::input(key(PackedQueryEdge::INGREDIENT_MASK + 1, 10_842_123, 42));
        let mut origin = QueryOrigin::derived([packed]);

        origin.set_edges([wide]).unwrap();
        assert!(!origin.as_ref().edges().is_packed());

        origin.set_edges([packed]).unwrap();
        assert!(origin.as_ref().edges().is_packed());
    }

    #[cfg(feature = "persistence")]
    #[test]
    fn query_origin_serde_round_trip_preserves_edges() {
        let input = QueryEdge::input(key(231, 10_842_122, 41));
        let output = QueryEdge::output(key(232, 10_842_123, PackedQueryEdge::GENERATION_MASK + 1));
        let origin = QueryOrigin::derived_untracked([input, output]);
        let serialized = serde_json::to_string(&origin).unwrap();
        let deserialized: QueryOrigin = serde_json::from_str(&serialized).unwrap();
        let QueryOriginRef::DerivedUntracked(edges) = deserialized.as_ref() else {
            panic!("expected untracked derived origin");
        };

        assert!(!edges.is_packed());
        assert_eq!(edges.iter().collect::<Vec<_>>(), vec![input, output]);
        assert_eq!(
            edges.iter().map(QueryEdge::kind).collect::<Vec<_>>(),
            vec![QueryEdgeKind::Input, QueryEdgeKind::Output]
        );
    }

    fn key(ingredient: u32, index: u32, generation: u32) -> DatabaseKeyIndex {
        // SAFETY: Test inputs are valid `Id` indices.
        let id = unsafe { Id::from_index(index) }.with_generation(generation);

        DatabaseKeyIndex::new(IngredientIndex::new(ingredient), id)
    }
}
