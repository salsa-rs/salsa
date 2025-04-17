use std::cell::RefCell;
use std::panic::UnwindSafe;
use std::sync::atomic::AtomicBool;

use rustc_hash::FxHashMap;
use tracing::debug;

use crate::accumulator::accumulated_map::{AccumulatedMap, AtomicInputAccumulatedValues};
use crate::active_query::QueryStack;
use crate::cycle::CycleHeads;
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::runtime::Stamp;
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
        iteration_count: u32,
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

    /// Executes a closure within the context of the current active query stacks.
    #[inline(always)]
    pub(crate) fn with_query_stack<R>(
        &self,
        c: impl UnwindSafe + FnOnce(&mut QueryStack) -> R,
    ) -> R {
        c(&mut self.query_stack.borrow_mut())
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
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_output(entity)
            }
        })
    }

    /// Check whether `entity` is an output of the currently active query (if any)
    pub(crate) fn is_output_of_active_query(&self, entity: DatabaseKeyIndex) -> bool {
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
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
        self.with_query_stack(|stack| {
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
            top_query.tracked_struct_ids.get(identity)
        })
    }

    #[track_caller]
    pub(crate) fn store_tracked_struct_id(&self, identity: Identity, id: Id) {
        self.with_query_stack(|stack| {
            let top_query = stack
                .last_mut()
                .expect("cannot store a tracked struct ID outside of a tracked function");
            let old_id = top_query.tracked_struct_ids.insert(identity, id);
            assert!(
                old_id.is_none(),
                "overwrote a previous id for `{identity:?}`"
            );
        })
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self, current_revision: Revision) {
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
    pub(super) tracked_struct_ids: IdentityMap,

    pub(super) accumulated: Option<Box<AccumulatedMap>>,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any direct or indirect accumulated values.
    pub(super) accumulated_inputs: AtomicInputAccumulatedValues,

    /// Are the `cycle_heads` verified to not be provisional anymore?
    pub(super) verified_final: AtomicBool,

    /// This result was computed based on provisional values from
    /// these cycle heads. The "cycle head" is the query responsible
    /// for managing a fixpoint iteration. In a cycle like
    /// `--> A --> B --> C --> A`, the cycle head is query `A`: it is
    /// the query whose value is requested while it is executing,
    /// which must provide the initial provisional value and decide,
    /// after each iteration, whether the cycle has converged or must
    /// iterate again.
    pub(super) cycle_heads: CycleHeads,
}

impl QueryRevisions {
    pub(crate) fn fixpoint_initial(query: DatabaseKeyIndex, revision: Revision) -> Self {
        Self {
            changed_at: revision,
            durability: Durability::MAX,
            origin: QueryOrigin::FixpointInitial,
            tracked_struct_ids: Default::default(),
            accumulated: Default::default(),
            accumulated_inputs: Default::default(),
            verified_final: AtomicBool::new(false),
            cycle_heads: CycleHeads::initial(query),
        }
    }
}

/// Tracks the way that a memoized value for a query was created.
#[derive(Debug, Clone)]
pub enum QueryOrigin {
    /// The value was assigned as the output of another query (e.g., using `specify`).
    /// The `DatabaseKeyIndex` is the identity of the assigning query.
    Assigned(DatabaseKeyIndex),

    /// The value was derived by executing a function
    /// and we were able to track ALL of that function's inputs.
    /// Those inputs are described in [`QueryEdges`].
    Derived(QueryEdges),

    /// The value was derived by executing a function
    /// but that function also reported that it read untracked inputs.
    /// The [`QueryEdges`] argument contains a listing of all the inputs we saw
    /// (but we know there were more).
    DerivedUntracked(QueryEdges),

    /// The value is an initial provisional value for a query that supports fixpoint iteration.
    FixpointInitial,
}

impl QueryOrigin {
    /// Indices for queries *read* by this query
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::FixpointInitial => None,
        };
        opt_edges.into_iter().flat_map(|edges| edges.inputs())
    }

    /// Indices for queries *written* by this query (if any)
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::FixpointInitial => None,
        };
        opt_edges.into_iter().flat_map(|edges| edges.outputs())
    }
}

/// The edges between a memoized value and other queries in the dependency graph.
/// These edges include both dependency edges
/// e.g., when creating the memoized value for Q0 executed another function Q1)
/// and output edges
/// (e.g., when Q0 specified the value for another query Q2).
#[derive(Debug, Clone)]
pub struct QueryEdges {
    /// The list of outgoing edges from this node.
    /// This list combines *both* inputs and outputs.
    ///
    /// Note that we always track input dependencies even when there are untracked reads.
    /// Untracked reads mean that we can't verify values, so we don't use the list of inputs for that,
    /// but we still use it for finding the transitive inputs to an accumulator.
    ///
    /// You can access the input/output list via the methods [`inputs`] and [`outputs`] respectively.
    ///
    /// Important:
    ///
    /// * The inputs must be in **execution order** for the red-green algorithm to work.
    // pub input_outputs: ThinBox<[DependencyEdge]>, once that is a thing
    pub input_outputs: Box<[QueryEdge]>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QueryEdge {
    Input(DatabaseKeyIndex),
    Output(DatabaseKeyIndex),
}

impl QueryEdges {
    /// Returns the (tracked) inputs that were executed in computing this memoized value.
    ///
    /// These will always be in execution order.
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        self.input_outputs.iter().filter_map(|&edge| match edge {
            QueryEdge::Input(dependency_index) => Some(dependency_index),
            QueryEdge::Output(_) => None,
        })
    }

    /// Returns the (tracked) outputs that were executed in computing this memoized value.
    ///
    /// These will always be in execution order.
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DatabaseKeyIndex> + '_ {
        self.input_outputs.iter().filter_map(|&edge| match edge {
            QueryEdge::Output(dependency_index) => Some(dependency_index),
            QueryEdge::Input(_) => None,
        })
    }

    /// Creates a new `QueryEdges`; the values given for each field must meet struct invariants.
    pub(crate) fn new(input_outputs: impl IntoIterator<Item = QueryEdge>) -> Self {
        Self {
            input_outputs: input_outputs.into_iter().collect(),
        }
    }
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
        self.local_state.with_query_stack(|stack| {
            #[cfg(debug_assertions)]
            assert_eq!(stack.len(), self.push_len);
            let frame = stack.last_mut().unwrap();
            assert!(frame.tracked_struct_ids.is_empty());
            frame.tracked_struct_ids.clone_from(tracked_struct_ids);
        })
    }

    /// Invoked when the query has successfully completed execution.
    fn complete(self) -> QueryRevisions {
        let query = self.local_state.with_query_stack(|stack| {
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
        self.local_state.with_query_stack(|stack| {
            stack.pop(
                self.database_key_index,
                #[cfg(debug_assertions)]
                self.push_len,
            );
        });
    }
}
