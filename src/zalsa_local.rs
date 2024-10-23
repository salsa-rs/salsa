use rustc_hash::{FxHashMap, FxHashSet};
use tracing::debug;

use crate::accumulator::accumulated_map::AccumulatedMap;
use crate::active_query::ActiveQuery;
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::key::DependencyIndex;
use crate::runtime::StampedValue;
use crate::table::PageIndex;
use crate::table::Slot;
use crate::table::Table;
use crate::tracked_struct::{Disambiguator, Identity, IdentityHash};
use crate::zalsa::IngredientIndex;
use crate::Accumulator;
use crate::Cancelled;
use crate::Database;
use crate::Event;
use crate::EventKind;
use crate::Id;
use crate::Revision;
use std::cell::RefCell;
use std::sync::Arc;

/// State that is specific to a single execution thread.
///
/// Internally, this type uses ref-cells.
///
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
pub struct ZalsaLocal {
    /// Vector of active queries.
    ///
    /// This is normally `Some`, but it is set to `None`
    /// while the query is blocked waiting for a result.
    ///
    /// Unwinding note: pushes onto this vector must be popped -- even
    /// during unwinding.
    query_stack: RefCell<Option<Vec<ActiveQuery>>>,

    /// Stores the most recent page for a given ingredient.
    /// This is thread-local to avoid contention.
    most_recent_pages: RefCell<FxHashMap<IngredientIndex, PageIndex>>,
}

impl ZalsaLocal {
    pub(crate) fn new() -> Self {
        ZalsaLocal {
            query_stack: RefCell::new(Some(vec![])),
            most_recent_pages: RefCell::new(FxHashMap::default()),
        }
    }

    /// Allocate a new id in `table` for the given ingredient
    /// storing `value`. Remembers the most recent page from this
    /// thread and attempts to reuse it.
    pub(crate) fn allocate<T: Slot>(
        &self,
        table: &Table,
        ingredient: IngredientIndex,
        mut value: impl FnOnce() -> T,
    ) -> Id {
        // Find the most recent page, pushing a page if needed
        let mut page = *self
            .most_recent_pages
            .borrow_mut()
            .entry(ingredient)
            .or_insert_with(|| table.push_page::<T>(ingredient));

        loop {
            // Try to allocate an entry on that page
            let page_ref = table.page::<T>(page);
            match page_ref.allocate(page, value) {
                // If succesful, return
                Ok(id) => return id,

                // Otherwise, create a new page and try again
                Err(v) => {
                    value = v;
                    page = table.push_page::<T>(ingredient);
                    self.most_recent_pages.borrow_mut().insert(ingredient, page);
                }
            }
        }
    }

    #[inline]
    pub(crate) fn push_query(&self, database_key_index: DatabaseKeyIndex) -> ActiveQueryGuard<'_> {
        let mut query_stack = self.query_stack.borrow_mut();
        let query_stack = query_stack.as_mut().expect("local stack taken");
        query_stack.push(ActiveQuery::new(database_key_index));
        ActiveQueryGuard {
            local_state: self,
            database_key_index,
            push_len: query_stack.len(),
        }
    }

    fn with_query_stack<R>(&self, c: impl FnOnce(&mut Vec<ActiveQuery>) -> R) -> R {
        c(self
            .query_stack
            .borrow_mut()
            .as_mut()
            .expect("query stack taken"))
    }

    fn query_in_progress(&self) -> bool {
        self.with_query_stack(|stack| !stack.is_empty())
    }

    /// Returns the index of the active query along with its *current* durability/changed-at
    /// information. As the query continues to execute, naturally, that information may change.
    pub(crate) fn active_query(&self) -> Option<(DatabaseKeyIndex, StampedValue<()>)> {
        self.with_query_stack(|stack| {
            stack.last().map(|active_query| {
                (
                    active_query.database_key_index,
                    StampedValue {
                        value: (),
                        durability: active_query.durability,
                        changed_at: active_query.changed_at,
                    },
                )
            })
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
                top_query.accumulated.accumulate(index, value);
                Ok(())
            } else {
                Err(())
            }
        })
    }

    /// Add an output to the current query's list of dependencies
    pub(crate) fn add_output(&self, entity: DependencyIndex) {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_output(entity)
            }
        })
    }

    /// Check whether `entity` is an output of the currently active query (if any)
    pub(crate) fn is_output_of_active_query(&self, entity: DependencyIndex) -> bool {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.is_output(entity)
            } else {
                false
            }
        })
    }

    /// Register that currently active query reads the given input
    pub(crate) fn report_tracked_read(
        &self,
        input: DependencyIndex,
        durability: Durability,
        changed_at: Revision,
        cycle_heads: &FxHashSet<DatabaseKeyIndex>,
    ) {
        debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input, durability, changed_at
        );
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_read(input, durability, changed_at, cycle_heads);
            }
        })
    }

    /// Register that the current query read an untracked value
    ///
    /// # Parameters
    ///
    /// * `current_revision`, the current revision
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

    /// Takes the query stack and returns it. This is used when
    /// the current thread is blocking. The stack must be restored
    /// with [`Self::restore_query_stack`] when the thread unblocks.
    pub(crate) fn take_query_stack(&self) -> Vec<ActiveQuery> {
        assert!(
            self.query_stack.borrow().is_some(),
            "query stack already taken"
        );
        self.query_stack.take().unwrap()
    }

    /// Restores a query stack taken with [`Self::take_query_stack`] once
    /// the thread unblocks.
    pub(crate) fn restore_query_stack(&self, stack: Vec<ActiveQuery>) {
        assert!(self.query_stack.borrow().is_none(), "query stack not taken");
        self.query_stack.replace(Some(stack));
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
    pub(crate) fn disambiguate(&self, key: IdentityHash) -> (StampedValue<()>, Disambiguator) {
        assert!(
            self.query_in_progress(),
            "cannot create a tracked struct disambiguator outside of a tracked function"
        );

        self.with_query_stack(|stack| {
            let top_query = stack.last_mut().unwrap();
            let disambiguator = top_query.disambiguate(key);
            (
                StampedValue {
                    value: (),
                    durability: top_query.durability,
                    changed_at: top_query.changed_at,
                },
                disambiguator,
            )
        })
    }

    #[track_caller]
    pub(crate) fn tracked_struct_id(&self, identity: &Identity) -> Option<Id> {
        debug_assert!(
            self.query_in_progress(),
            "cannot create a tracked struct disambiguator outside of a tracked function"
        );

        self.with_query_stack(|stack| {
            let top_query = stack.last().unwrap();
            top_query.tracked_struct_ids.get(identity).copied()
        })
    }

    #[track_caller]
    pub(crate) fn store_tracked_struct_id(&self, identity: Identity, id: Id) {
        debug_assert!(
            self.query_in_progress(),
            "cannot create a tracked struct disambiguator outside of a tracked function"
        );
        self.with_query_stack(|stack| {
            let top_query = stack.last_mut().unwrap();
            let old_id = top_query.tracked_struct_ids.insert(identity, id);
            assert!(
                old_id.is_none(),
                "overwrote a previous id for `{identity:?}`"
            );
        })
    }

    /// Starts unwinding the stack if the current revision is cancelled.
    ///
    /// This method can be called by query implementations that perform
    /// potentially expensive computations, in order to speed up propagation of
    /// cancellation.
    ///
    /// Cancellation will automatically be triggered by salsa on any query
    /// invocation.
    ///
    /// This method should not be overridden by `Database` implementors. A
    /// `salsa_event` is emitted when this method is called, so that should be
    /// used instead.
    pub(crate) fn unwind_if_revision_cancelled(&self, db: &dyn Database) {
        let thread_id = std::thread::current().id();
        db.salsa_event(&|| Event {
            thread_id,

            kind: EventKind::WillCheckCancellation,
        });
        let zalsa = db.zalsa();
        if zalsa.load_cancellation_flag() {
            self.unwind_cancelled(zalsa.current_revision());
        }
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self, current_revision: Revision) {
        self.report_untracked_read(current_revision);
        Cancelled::PendingWrite.throw();
    }
}

impl std::panic::RefUnwindSafe for ZalsaLocal {}

/// Summarizes "all the inputs that a query used"
/// and "all the outputs its wrote to"
#[derive(Debug, Clone)]
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
    pub(super) tracked_struct_ids: FxHashMap<Identity, Id>,

    pub(super) accumulated: AccumulatedMap,

    /// This result was computed based on provisional cycle-iteration
    /// results from these queries.
    pub(super) cycle_heads: FxHashSet<DatabaseKeyIndex>,
}

impl QueryRevisions {
    pub(crate) fn fixpoint_initial(query: DatabaseKeyIndex) -> Self {
        let mut cycle_heads = FxHashSet::default();
        cycle_heads.insert(query);
        Self {
            changed_at: Revision::start(),
            durability: Durability::MAX,
            origin: QueryOrigin::FixpointInitial,
            tracked_struct_ids: Default::default(),
            accumulated: Default::default(),
            cycle_heads,
        }
    }

    pub(crate) fn stamped_value<V>(&self, value: V) -> StampedValue<V> {
        self.stamp_template().stamp(value)
    }

    pub(crate) fn stamp_template(&self) -> StampTemplate {
        StampTemplate {
            durability: self.durability,
            changed_at: self.changed_at,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct StampTemplate {
    durability: Durability,
    changed_at: Revision,
}

impl StampTemplate {
    pub(crate) fn stamp<V>(self, value: V) -> StampedValue<V> {
        StampedValue {
            value,
            durability: self.durability,
            changed_at: self.changed_at,
        }
    }
}

/// Tracks the way that a memoized value for a query was created.
#[derive(Debug, Clone)]
pub enum QueryOrigin {
    /// The value was assigned as the output of another query (e.g., using `specify`).
    /// The `DatabaseKeyIndex` is the identity of the assigning query.
    Assigned(DatabaseKeyIndex),

    /// This value was set as a base input to the computation.
    BaseInput,

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
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::BaseInput | QueryOrigin::FixpointInitial => {
                None
            }
        };
        opt_edges.into_iter().flat_map(|edges| edges.inputs())
    }

    /// Indices for queries *written* by this query (if any)
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::BaseInput | QueryOrigin::FixpointInitial => {
                None
            }
        };
        opt_edges.into_iter().flat_map(|edges| edges.outputs())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum EdgeKind {
    Input,
    Output,
}

lazy_static::lazy_static! {
    pub(crate) static ref EMPTY_DEPENDENCIES: Arc<[(EdgeKind, DependencyIndex)]> = Arc::new([]);
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
    pub input_outputs: Arc<[(EdgeKind, DependencyIndex)]>,
}

impl QueryEdges {
    /// Returns the (tracked) inputs that were executed in computing this memoized value.
    ///
    /// These will always be in execution order.
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        self.input_outputs
            .iter()
            .filter(|(edge_kind, _)| *edge_kind == EdgeKind::Input)
            .map(|(_, dependency_index)| *dependency_index)
    }

    /// Returns the (tracked) outputs that were executed in computing this memoized value.
    ///
    /// These will always be in execution order.
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        self.input_outputs
            .iter()
            .filter(|(edge_kind, _)| *edge_kind == EdgeKind::Output)
            .map(|(_, dependency_index)| *dependency_index)
    }

    /// Creates a new `QueryEdges`; the values given for each field must meet struct invariants.
    pub(crate) fn new(input_outputs: Arc<[(EdgeKind, DependencyIndex)]>) -> Self {
        Self { input_outputs }
    }
}

/// When a query is pushed onto the `active_query` stack, this guard
/// is returned to represent its slot. The guard can be used to pop
/// the query from the stack -- in the case of unwinding, the guard's
/// destructor will also remove the query.
pub(crate) struct ActiveQueryGuard<'me> {
    local_state: &'me ZalsaLocal,
    push_len: usize,
    pub(crate) database_key_index: DatabaseKeyIndex,
}

impl ActiveQueryGuard<'_> {
    fn pop_helper(&self) -> ActiveQuery {
        self.local_state.with_query_stack(|stack| {
            // Sanity check: pushes and pops should be balanced.
            assert_eq!(stack.len(), self.push_len);
            debug_assert_eq!(
                stack.last().unwrap().database_key_index,
                self.database_key_index
            );
            stack.pop().unwrap()
        })
    }

    /// Initialize the tracked struct ids with the values from the prior execution.
    pub(crate) fn seed_tracked_struct_ids(&self, tracked_struct_ids: &FxHashMap<Identity, Id>) {
        self.local_state.with_query_stack(|stack| {
            assert_eq!(stack.len(), self.push_len);
            let frame = stack.last_mut().unwrap();
            assert!(frame.tracked_struct_ids.is_empty());
            frame.tracked_struct_ids = tracked_struct_ids.clone();
        })
    }

    /// Invoked when the query has successfully completed execution.
    pub(crate) fn complete(self) -> ActiveQuery {
        let query = self.pop_helper();
        std::mem::forget(self);
        query
    }

    /// Pops an active query from the stack. Returns the [`QueryRevisions`]
    /// which summarizes the other queries that were accessed during this
    /// query's execution.
    #[inline]
    pub(crate) fn pop(self) -> QueryRevisions {
        // Extract accumulated inputs.
        let popped_query = self.complete();

        popped_query.into_revisions()
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.pop_helper();
    }
}
