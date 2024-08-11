use tracing::debug;

use crate::active_query::ActiveQuery;
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::key::DependencyIndex;
use crate::runtime::StampedValue;
use crate::tracked_struct::Disambiguator;
use crate::zalsa::IngredientIndex;
use crate::Cancelled;
use crate::Cycle;
use crate::Database;
use crate::Event;
use crate::EventKind;
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
}

impl ZalsaLocal {
    pub(crate) fn new() -> Self {
        ZalsaLocal {
            query_stack: RefCell::new(Some(vec![])),
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
    ) {
        debug!(
            "report_tracked_read(input={:?}, durability={:?}, changed_at={:?})",
            input, durability, changed_at
        );
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_read(input, durability, changed_at);

                // We are a cycle participant:
                //
                //     C0 --> ... --> Ci --> Ci+1 -> ... -> Cn --> C0
                //                        ^   ^
                //                        :   |
                //         This edge -----+   |
                //                            |
                //                            |
                //                            N0
                //
                // In this case, the value we have just read from `Ci+1`
                // is actually the cycle fallback value and not especially
                // interesting. We unwind now with `CycleParticipant` to avoid
                // executing the rest of our query function. This unwinding
                // will be caught and our own fallback value will be used.
                //
                // Note that `Ci+1` may` have *other* callers who are not
                // participants in the cycle (e.g., N0 in the graph above).
                // They will not have the `cycle` marker set in their
                // stack frames, so they will just read the fallback value
                // from `Ci+1` and continue on their merry way.
                if let Some(cycle) = &top_query.cycle {
                    cycle.clone().throw()
                }
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
    pub(crate) fn disambiguate(
        &self,
        entity_index: IngredientIndex,
        reset_at: Revision,
        data_hash: u64,
    ) -> (DatabaseKeyIndex, StampedValue<()>, Disambiguator) {
        assert!(
            self.query_in_progress(),
            "cannot create a tracked struct disambiguator outside of a tracked function"
        );

        self.report_tracked_read(
            DependencyIndex::for_table(entity_index),
            Durability::MAX,
            reset_at,
        );

        self.with_query_stack(|stack| {
            let top_query = stack.last_mut().unwrap();
            let disambiguator = top_query.disambiguate(data_hash);
            (
                top_query.database_key_index,
                StampedValue {
                    value: (),
                    durability: top_query.durability,
                    changed_at: top_query.changed_at,
                },
                disambiguator,
            )
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
#[derive(Debug, Clone)]
pub(crate) struct QueryRevisions {
    /// The most revision in which some input changed.
    pub(crate) changed_at: Revision,

    /// Minimum durability of the inputs to this query.
    pub(crate) durability: Durability,

    /// How was this query computed?
    pub(crate) origin: QueryOrigin,
}

impl QueryRevisions {
    pub(crate) fn stamped_value<V>(&self, value: V) -> StampedValue<V> {
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
}

impl QueryOrigin {
    /// Indices for queries *read* by this query
    pub(crate) fn inputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::BaseInput => None,
        };
        opt_edges.into_iter().flat_map(|edges| edges.inputs())
    }

    /// Indices for queries *written* by this query (if any)
    pub(crate) fn outputs(&self) -> impl DoubleEndedIterator<Item = DependencyIndex> + '_ {
        let opt_edges = match self {
            QueryOrigin::Derived(edges) | QueryOrigin::DerivedUntracked(edges) => Some(edges),
            QueryOrigin::Assigned(_) | QueryOrigin::BaseInput => None,
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

        // If this frame were a cycle participant, it would have unwound.
        assert!(popped_query.cycle.is_none());

        popped_query.revisions()
    }

    /// If the active query is registered as a cycle participant, remove and
    /// return that cycle.
    pub(crate) fn take_cycle(&self) -> Option<Cycle> {
        self.local_state
            .with_query_stack(|stack| stack.last_mut()?.cycle.take())
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.pop_helper();
    }
}
