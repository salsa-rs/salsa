use std::cell::RefCell;
use std::panic::UnwindSafe;
use std::ptr::{self, NonNull};

use rustc_hash::FxHashMap;
use tracing::debug;

use crate::accumulator::accumulated_map::{AccumulatedMap, AtomicInputAccumulatedValues};
use crate::active_query::QueryStack;
use crate::cycle::{empty_cycle_heads, CycleHeads, IterationCount};
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::runtime::Stamp;
use crate::sync::atomic::AtomicBool;
use crate::table::{PageIndex, Slot, Table};
use crate::tracked_struct::{Disambiguator, Identity, IdentityHash, IdentityMap};
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Accumulator, Cancelled, Id, Revision};

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
    most_recent_pages: RefCell<FxHashMap<IngredientIndex, PageIndex>>,
}

impl ZalsaLocal {
    pub(crate) fn new() -> Self {
        ZalsaLocal {
            query_stack: RefCell::new(QueryStack::default()),
            most_recent_pages: RefCell::new(FxHashMap::default()),
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
    pub(crate) fn allocate<T: Slot>(
        &self,
        zalsa: &Zalsa,
        ingredient: IngredientIndex,
        mut value: impl FnOnce(Id) -> T,
    ) -> Id {
        let memo_types = || {
            zalsa
                .lookup_ingredient(ingredient)
                .memo_table_types()
                .clone()
        };
        // Find the most recent page, pushing a page if needed
        let mut page = *self
            .most_recent_pages
            .borrow_mut()
            .entry(ingredient)
            .or_insert_with(|| {
                zalsa
                    .table()
                    .fetch_or_push_page::<T>(ingredient, memo_types)
            });

        loop {
            // Try to allocate an entry on that page
            let page_ref = zalsa.table().page::<T>(page);
            match page_ref.allocate(page, value) {
                // If successful, return
                Ok(id) => return id,

                // Otherwise, create a new page and try again
                // Note that we could try fetching a page again, but as we just filled one up
                // it is unlikely that there is a non-full one available.
                Err(v) => {
                    value = v;
                    page = zalsa.table().push_page::<T>(ingredient, memo_types());
                    self.most_recent_pages.borrow_mut().insert(ingredient, page);
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
        let mut query_stack = self.query_stack.borrow_mut();
        query_stack.push_new_query(database_key_index, iteration_count);
        ActiveQueryGuard {
            local_state: self,
            database_key_index,
            #[cfg(debug_assertions)]
            push_len: query_stack.len(),
        }
    }

    /// Executes a closure within the context of the current active query stacks (mutable).
    #[inline(always)]
    pub(crate) fn with_query_stack_mut<R>(
        &self,
        c: impl UnwindSafe + FnOnce(&mut QueryStack) -> R,
    ) -> R {
        c(&mut self.query_stack.borrow_mut())
    }

    #[inline(always)]
    pub(crate) fn with_query_stack<R>(&self, c: impl UnwindSafe + FnOnce(&QueryStack) -> R) -> R {
        c(&mut self.query_stack.borrow())
    }

    #[inline(always)]
    pub(crate) fn try_with_query_stack<R>(
        &self,
        c: impl UnwindSafe + FnOnce(&QueryStack) -> R,
    ) -> Option<R> {
        self.query_stack
            .try_borrow()
            .ok()
            .as_ref()
            .map(|stack| c(stack))
    }

    /// Returns the index of the active query along with its *current* durability/changed-at
    /// information. As the query continues to execute, naturally, that information may change.
    pub(crate) fn active_query(&self) -> Option<(DatabaseKeyIndex, Stamp)> {
        self.with_query_stack(|stack| {
            stack
                .last()
                .map(|active_query| (active_query.database_key_index, active_query.stamp()))
        })
    }

    /// Add an output to the current query's list of dependencies
    ///
    /// Returns `Err` if not in a query.
    pub(crate) fn accumulate<A: Accumulator>(
        &self,
        index: IngredientIndex,
        value: A,
    ) -> Result<(), ()> {
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.accumulate(index, value);
                Ok(())
            } else {
                Err(())
            }
        })
    }

    /// Add an output to the current query's list of dependencies
    pub(crate) fn add_output(&self, entity: DatabaseKeyIndex) {
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_output(entity)
            }
        })
    }

    /// Check whether `entity` is an output of the currently active query (if any)
    pub(crate) fn is_output_of_active_query(&self, entity: DatabaseKeyIndex) -> bool {
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.is_output(entity)
            } else {
                false
            }
        })
    }

    /// Register that currently active query reads the given input
    #[inline(always)]
    pub(crate) fn report_tracked_read(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
        has_accumulated: bool,
        accumulated_inputs: &AtomicInputAccumulatedValues,
        cycle_heads: &CycleHeads,
    ) {
        debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input, durability, changed_at
        );
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_read(
                    input,
                    durability,
                    changed_at,
                    has_accumulated,
                    accumulated_inputs,
                    cycle_heads,
                );
            }
        })
    }

    /// Register that currently active query reads the given input
    #[inline(always)]
    pub(crate) fn report_tracked_read_simple(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
    ) {
        debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input, durability, changed_at
        );
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_read_simple(input, durability, changed_at);
            }
        })
    }

    /// Register that the current query read an untracked value
    ///
    /// # Parameters
    ///
    /// * `current_revision`, the current revision
    #[inline(always)]
    pub(crate) fn report_untracked_read(&self, current_revision: Revision) {
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_untracked_read(current_revision);
            }
        })
    }

    /// Update the top query on the stack to act as though it read a value
    /// of durability `durability` which changed in `revision`.
    // FIXME: Use or remove this.
    #[allow(dead_code)]
    pub(crate) fn report_synthetic_read(&self, durability: Durability, revision: Revision) {
        self.with_query_stack_mut(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_synthetic_read(durability, revision);
            }
        })
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
        self.with_query_stack_mut(|stack| {
            let top_query = stack.last_mut().expect(
                "cannot create a tracked struct disambiguator outside of a tracked function",
            );
            let disambiguator = top_query.disambiguate(key);
            (top_query.stamp(), disambiguator)
        })
    }

    #[track_caller]
    pub(crate) fn tracked_struct_id(&self, identity: &Identity) -> Option<Id> {
        self.with_query_stack(|stack| {
            let top_query = stack
                .last()
                .expect("cannot create a tracked struct ID outside of a tracked function");
            top_query.tracked_struct_ids().get(identity)
        })
    }

    #[track_caller]
    pub(crate) fn store_tracked_struct_id(&self, identity: Identity, id: Id) {
        self.with_query_stack_mut(|stack| {
            let top_query = stack
                .last_mut()
                .expect("cannot store a tracked struct ID outside of a tracked function");
            top_query.tracked_struct_ids_mut().insert(identity, id);
        })
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self, current_revision: Revision) {
        // Why is this reporting an untracked read? We do not store the query revisions on unwind do we?
        self.report_untracked_read(current_revision);
        Cancelled::PendingWrite.throw();
    }
}

// Okay to implement as `ZalsaLocal`` is !Sync
// - `most_recent_pages` can't observe broken states as we cannot panic such that we enter an
//   inconsistent state
// - neither can `query_stack` as we require the closures accessing it to be `UnwindSafe`
impl std::panic::RefUnwindSafe for ZalsaLocal {}

/// Summarizes "all the inputs that a query used"
/// and "all the outputs it has written to"
#[derive(Debug)]
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
    pub(super) accumulated_inputs: AtomicInputAccumulatedValues,

    /// Are the `cycle_heads` verified to not be provisional anymore?
    ///
    /// Note that this field could be in `QueryRevisionsExtra` as it is only
    /// relevant for queries that participate in a cycle, but we get it for
    /// free anyways due to padding.
    pub(super) verified_final: AtomicBool,

    /// Lazily allocated state.
    pub(super) extra: QueryRevisionsExtra,
}

/// Data on `QueryRevisions` that is lazily allocated to save memory
/// in the common case.
///
/// In particular, not all queries create tracked structs, participate
/// in cycles, or create accumulators.
#[derive(Debug, Default)]
pub(crate) struct QueryRevisionsExtra(Option<Box<QueryRevisionsExtraInner>>);

impl QueryRevisionsExtra {
    pub fn new(
        accumulated: AccumulatedMap,
        tracked_struct_ids: IdentityMap,
        cycle_heads: CycleHeads,
        iteration: IterationCount,
    ) -> Self {
        let inner = if tracked_struct_ids.is_empty()
            && cycle_heads.is_empty()
            && accumulated.is_empty()
            && iteration.is_initial()
        {
            None
        } else {
            Some(Box::new(QueryRevisionsExtraInner {
                accumulated,
                cycle_heads,
                tracked_struct_ids,
                iteration,
            }))
        };

        Self(inner)
    }
}

#[derive(Debug)]
struct QueryRevisionsExtraInner {
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
    tracked_struct_ids: IdentityMap,

    /// This result was computed based on provisional values from
    /// these cycle heads. The "cycle head" is the query responsible
    /// for managing a fixpoint iteration. In a cycle like
    /// `--> A --> B --> C --> A`, the cycle head is query `A`: it is
    /// the query whose value is requested while it is executing,
    /// which must provide the initial provisional value and decide,
    /// after each iteration, whether the cycle has converged or must
    /// iterate again.
    cycle_heads: CycleHeads,

    iteration: IterationCount,
}

#[cfg(not(feature = "shuttle"))]
#[cfg(target_pointer_width = "64")]
const _: [(); std::mem::size_of::<QueryRevisions>()] = [(); std::mem::size_of::<[usize; 4]>()];

#[cfg(not(feature = "shuttle"))]
#[cfg(target_pointer_width = "64")]
const _: [(); std::mem::size_of::<QueryRevisionsExtraInner>()] =
    [(); std::mem::size_of::<[usize; 10]>()];

impl QueryRevisions {
    pub(crate) fn fixpoint_initial(query: DatabaseKeyIndex) -> Self {
        Self {
            changed_at: Revision::start(),
            durability: Durability::MAX,
            origin: QueryOrigin::fixpoint_initial(),
            accumulated_inputs: Default::default(),
            verified_final: AtomicBool::new(false),
            extra: QueryRevisionsExtra::new(
                AccumulatedMap::default(),
                IdentityMap::default(),
                CycleHeads::initial(query),
                IterationCount::initial(),
            ),
        }
    }

    /// Returns a reference to the `AccumulatedMap` for this query, or `None` if the map is empty.
    pub(crate) fn accumulated(&self) -> Option<&AccumulatedMap> {
        self.extra
            .0
            .as_ref()
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

    /// Returns a mutable reference to the `CycleHeads` for this query, or `None` if the list is empty.
    pub(crate) fn cycle_heads_mut(&mut self) -> Option<&mut CycleHeads> {
        self.extra
            .0
            .as_mut()
            .map(|extra| &mut extra.cycle_heads)
            .filter(|cycle_heads| !cycle_heads.is_empty())
    }

    /// Sets the `CycleHeads` for this query.
    pub(crate) fn set_cycle_heads(&mut self, cycle_heads: CycleHeads) {
        match &mut self.extra.0 {
            Some(extra) => extra.cycle_heads = cycle_heads,
            None => {
                self.extra = QueryRevisionsExtra::new(
                    AccumulatedMap::default(),
                    IdentityMap::default(),
                    cycle_heads,
                    IterationCount::default(),
                );
            }
        };
    }

    pub(crate) const fn iteration(&self) -> IterationCount {
        match &self.extra.0 {
            Some(extra) => extra.iteration,
            None => IterationCount::initial(),
        }
    }

    /// Updates the iteration count if this query has any cycle heads. Otherwise it's a no-op.
    pub(crate) fn update_iteration_count(&mut self, iteration_count: IterationCount) {
        if let Some(extra) = &mut self.extra.0 {
            extra.iteration = iteration_count
        }
    }

    /// Returns a reference to the `IdentityMap` for this query, or `None` if the map is empty.
    pub fn tracked_struct_ids(&self) -> Option<&IdentityMap> {
        self.extra
            .0
            .as_ref()
            .map(|extra| &extra.tracked_struct_ids)
            .filter(|tracked_struct_ids| !tracked_struct_ids.is_empty())
    }

    /// Returns a mutable reference to the `IdentityMap` for this query, or `None` if the map is empty.
    pub fn tracked_struct_ids_mut(&mut self) -> Option<&mut IdentityMap> {
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
#[derive(Debug, Clone)]
pub enum QueryOriginRef<'a> {
    /// The value was assigned as the output of another query (e.g., using `specify`).
    /// The `DatabaseKeyIndex` is the identity of the assigning query.
    Assigned(DatabaseKeyIndex),

    /// The value was derived by executing a function
    /// and we were able to track ALL of that function's inputs.
    /// Those inputs are described in [`QueryEdges`].
    Derived(&'a [QueryEdge]),

    /// The value was derived by executing a function
    /// but that function also reported that it read untracked inputs.
    /// The [`QueryEdges`] argument contains a listing of all the inputs we saw
    /// (but we know there were more).
    DerivedUntracked(&'a [QueryEdge]),

    /// The value is an initial provisional value for a query that supports fixpoint iteration.
    FixpointInitial,
}

impl<'a> QueryOriginRef<'a> {
    /// Indices for queries *read* by this query
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        let opt_edges = match self {
            QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => Some(edges),
            QueryOriginRef::Assigned(_) | QueryOriginRef::FixpointInitial => None,
        };
        opt_edges.into_iter().flat_map(|edges| input_edges(edges))
    }

    /// Indices for queries *written* by this query (if any)
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        let opt_edges = match self {
            QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => Some(edges),
            QueryOriginRef::Assigned(_) | QueryOriginRef::FixpointInitial => None,
        };
        opt_edges.into_iter().flat_map(|edges| output_edges(edges))
    }

    pub(crate) fn edges(&self) -> &'a [QueryEdge] {
        let opt_edges = match self {
            QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => Some(edges),
            QueryOriginRef::Assigned(_) | QueryOriginRef::FixpointInitial => None,
        };

        opt_edges.copied().unwrap_or_default()
    }
}

#[derive(Clone, Copy)]
enum QueryOriginKind {
    /// The value was assigned as the output of another query.
    ///
    /// This can, for example, can occur when `specify` is used.
    Assigned,

    /// The value was derived by executing a function
    /// _and_ Salsa was able to track all of said function's inputs.
    Derived,

    /// The value was derived by executing a function
    /// but that function also reported that it read untracked inputs.
    DerivedUntracked,

    /// An initial provisional value.
    ///
    /// This will occur occur in queries that support fixpoint iteration.
    FixpointInitial,
}

/// Tracks how a memoized value for a given query was created.
///
/// This type is a manual enum packed to 13 bytes to reduce the size of `QueryRevisions`.
#[repr(Rust, packed)]
pub struct QueryOrigin {
    /// The tag of this enum.
    ///
    /// Note that this tag only requires two bits and could likely be packed into
    /// some other field. However, we get this byte for free thanks to alignment.
    kind: QueryOriginKind,

    /// The data portion of this enum.
    data: QueryOriginData,

    /// The metadata of this enum.
    ///
    /// For `QueryOriginKind::Derived` and `QueryOriginKind::DerivedUntracked`, this
    /// is the length of the `input_outputs` allocation.
    ///
    /// For `QueryOriginKind::Assigned`, this is the `IngredientIndex` of assigning query.
    /// Combined with the `Id` data, this forms a complete `DatabaseKeyIndex`.
    ///
    /// For `QueryOriginKind::FixpointInitial`, this field is zero.
    metadata: u32,
}

/// The data portion of `PackedQueryOrigin`.
union QueryOriginData {
    /// Query edges for `QueryOriginKind::Derived` or `QueryOriginKind::DerivedUntracked`.
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
    input_outputs: NonNull<QueryEdge>,

    /// The identity of the assigning query for `QueryOriginKind::Assigned`.
    index: Id,

    /// `QueryOriginKind::FixpointInitial` holds no data.
    empty: (),
}

/// SAFETY: The `input_outputs` pointer is owned and not accessed or shared concurrently.
unsafe impl Send for QueryOriginData {}
/// SAFETY: Same as above.
unsafe impl Sync for QueryOriginData {}

impl QueryOrigin {
    /// Create a query origin of type `QueryOriginKind::FixpointInitial`.
    pub fn fixpoint_initial() -> QueryOrigin {
        QueryOrigin {
            kind: QueryOriginKind::FixpointInitial,
            metadata: 0,
            data: QueryOriginData { empty: () },
        }
    }

    /// Create a query origin of type `QueryOriginKind::Derived`, with the given edges.
    pub fn derived(input_outputs: impl IntoIterator<Item = QueryEdge>) -> QueryOrigin {
        let input_outputs = input_outputs.into_iter().collect::<Box<[_]>>();

        // Exceeding `u32::MAX` query edges should never happen in real-world usage.
        let length = u32::try_from(input_outputs.len())
            .expect("exceeded more than `u32::MAX` query edges; this should never happen.");

        // SAFETY: `Box::into_raw` returns a non-null pointer.
        let input_outputs =
            unsafe { NonNull::new_unchecked(Box::into_raw(input_outputs).cast::<QueryEdge>()) };

        QueryOrigin {
            kind: QueryOriginKind::Derived,
            metadata: length,
            data: QueryOriginData { input_outputs },
        }
    }

    /// Create a query origin of type `QueryOriginKind::DerivedUntracked`, with the given edges.
    pub fn derived_untracked(input_outputs: impl IntoIterator<Item = QueryEdge>) -> QueryOrigin {
        let mut origin = QueryOrigin::derived(input_outputs);
        origin.kind = QueryOriginKind::DerivedUntracked;
        origin
    }

    /// Create a query origin of type `QueryOriginKind::Assigned`, with the given key.
    pub fn assigned(key: DatabaseKeyIndex) -> QueryOrigin {
        QueryOrigin {
            kind: QueryOriginKind::Assigned,
            metadata: key.ingredient_index().as_u32(),
            data: QueryOriginData {
                index: key.key_index(),
            },
        }
    }

    /// Return a read-only reference to this query origin.
    pub fn as_ref(&self) -> QueryOriginRef<'_> {
        match self.kind {
            QueryOriginKind::Assigned => {
                // SAFETY: `data.index` is initialized when the tag is `QueryOriginKind::Assigned`.
                let index = unsafe { self.data.index };
                let ingredient_index = IngredientIndex::from(self.metadata as usize);
                QueryOriginRef::Assigned(DatabaseKeyIndex::new(ingredient_index, index))
            }

            QueryOriginKind::Derived => {
                // SAFETY: `data.input_outputs` is initialized when the tag is `QueryOriginKind::Derived`.
                let input_outputs = unsafe { self.data.input_outputs };
                let length = self.metadata as usize;

                // SAFETY: `input_outputs` and `self.metadata` form a valid slice when the
                // tag is `QueryOriginKind::Derived`.
                let input_outputs =
                    unsafe { std::slice::from_raw_parts(input_outputs.as_ptr(), length) };

                QueryOriginRef::Derived(input_outputs)
            }

            QueryOriginKind::DerivedUntracked => {
                // SAFETY: `data.input_outputs` is initialized when the tag is `QueryOriginKind::DerivedUntracked`.
                let input_outputs = unsafe { self.data.input_outputs };
                let length = self.metadata as usize;

                // SAFETY: `input_outputs` and `self.metadata` form a valid slice when the
                // tag is `QueryOriginKind::DerivedUntracked`.
                let input_outputs =
                    unsafe { std::slice::from_raw_parts(input_outputs.as_ptr(), length) };

                QueryOriginRef::DerivedUntracked(input_outputs)
            }

            QueryOriginKind::FixpointInitial => QueryOriginRef::FixpointInitial,
        }
    }
}

impl Drop for QueryOrigin {
    fn drop(&mut self) {
        match self.kind {
            QueryOriginKind::Derived | QueryOriginKind::DerivedUntracked => {
                // SAFETY: `data.input_outputs` is initialized when the tag is `QueryOriginKind::Derived`
                // or `QueryOriginKind::DerivedUntracked`.
                let input_outputs = unsafe { self.data.input_outputs };
                let length = self.metadata as usize;

                // SAFETY: `input_outputs` and `self.metadata` form a valid slice when the
                // tag is `QueryOriginKind::DerivedUntracked` or `QueryOriginKind::DerivedUntracked`,
                // and we have `&mut self`.
                let _input_outputs: Box<[QueryEdge]> = unsafe {
                    Box::from_raw(ptr::slice_from_raw_parts_mut(
                        input_outputs.as_ptr(),
                        length,
                    ))
                };
            }

            // The data stored for this variants is `Copy`.
            QueryOriginKind::FixpointInitial | QueryOriginKind::Assigned => {}
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
/// This type is a packed version of `QueryEdgeKind`, tagging the `IngredientIndex`
/// in `key` with a discriminator for the input and output variants without increasing
/// the size of the type. Notably, this type is 12 bytes as opposed to the 16 byte
/// `QueryEdgeKind`, which is meaningful as inputs and outputs are stored contiguously.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct QueryEdge {
    key: DatabaseKeyIndex,
}

impl QueryEdge {
    /// Create an input query edge with the given index.
    pub fn input(key: DatabaseKeyIndex) -> QueryEdge {
        Self { key }
    }

    /// Create an output query edge with the given index.
    pub fn output(key: DatabaseKeyIndex) -> QueryEdge {
        let ingredient_index = key.ingredient_index().with_tag(true);

        Self {
            key: DatabaseKeyIndex::new(ingredient_index, key.key_index()),
        }
    }

    /// Returns the kind of this query edge.
    pub fn kind(self) -> QueryEdgeKind {
        // Clear the tag to restore the original index.
        let untagged = DatabaseKeyIndex::new(
            self.key.ingredient_index().with_tag(false),
            self.key.key_index(),
        );

        if self.key.ingredient_index().tag() {
            QueryEdgeKind::Output(untagged)
        } else {
            QueryEdgeKind::Input(untagged)
        }
    }
}

impl std::fmt::Debug for QueryEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.kind().fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QueryEdgeKind {
    Input(DatabaseKeyIndex),
    Output(DatabaseKeyIndex),
}

/// Returns the (tracked) inputs that were executed in computing this memoized value.
///
/// These will always be in execution order.
pub(crate) fn input_edges(
    input_outputs: &[QueryEdge],
) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
    input_outputs.iter().filter_map(|&edge| match edge.kind() {
        QueryEdgeKind::Input(dependency_index) => Some(dependency_index),
        QueryEdgeKind::Output(_) => None,
    })
}

/// Returns the (tracked) outputs that were executed in computing this memoized value.
///
/// These will always be in execution order.
pub(crate) fn output_edges(
    input_outputs: &[QueryEdge],
) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
    input_outputs.iter().filter_map(|&edge| match edge.kind() {
        QueryEdgeKind::Output(dependency_index) => Some(dependency_index),
        QueryEdgeKind::Input(_) => None,
    })
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
    pub(crate) fn seed_tracked_struct_ids(&self, tracked_struct_ids: &IdentityMap) {
        self.local_state.with_query_stack_mut(|stack| {
            #[cfg(debug_assertions)]
            assert_eq!(stack.len(), self.push_len);
            let frame = stack.last_mut().unwrap();
            assert!(frame.tracked_struct_ids().is_empty());
            frame
                .tracked_struct_ids_mut()
                .clone_from(tracked_struct_ids);
        })
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

        self.local_state.with_query_stack_mut(|stack| {
            #[cfg(debug_assertions)]
            assert_eq!(stack.len(), self.push_len);
            let frame = stack.last_mut().unwrap();
            frame.seed_iteration(durability, changed_at, edges, untracked_read);
        })
    }

    /// Invoked when the query has successfully completed execution.
    fn complete(self) -> QueryRevisions {
        let query = self.local_state.with_query_stack_mut(|stack| {
            stack.pop_into_revisions(
                self.database_key_index,
                #[cfg(debug_assertions)]
                self.push_len,
            )
        });
        std::mem::forget(self);
        query
    }

    /// Pops an active query from the stack. Returns the [`QueryRevisions`]
    /// which summarizes the other queries that were accessed during this
    /// query's execution.
    #[inline]
    pub(crate) fn pop(self) -> QueryRevisions {
        self.complete()
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.local_state.with_query_stack_mut(|stack| {
            stack.pop(
                self.database_key_index,
                #[cfg(debug_assertions)]
                self.push_len,
            );
        });
    }
}
