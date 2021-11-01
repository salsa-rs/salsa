use crate::durability::Durability;
use crate::runtime::ActiveQuery;
use crate::runtime::Revision;
use crate::Cycle;
use crate::Database;
use crate::DatabaseKeyIndex;
use crate::Event;
use crate::EventKind;
use std::cell::RefCell;
use std::sync::Arc;

/// State that is specific to a single execution thread.
///
/// Internally, this type uses ref-cells.
///
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
pub(super) struct LocalState {
    /// Vector of active queries.
    ///
    /// This is normally `Some`, but it is set to `None`
    /// while the query is blocked waiting for a result.
    ///
    /// Unwinding note: pushes onto this vector must be popped -- even
    /// during unwinding.
    query_stack: RefCell<Option<Vec<ActiveQuery>>>,
}

pub(crate) struct ComputedQueryResult<V> {
    /// Final value produced
    pub(crate) value: V,

    /// Information about the other queries that were
    /// accessed.
    pub(crate) revisions: QueryRevisions,

    /// If this node participated in a cycle, then this value is set
    /// to the cycle in which it participated.
    pub(crate) cycle_participant: Option<Cycle>,
}

/// Summarizes "all the inputs that a query used"
#[derive(Debug, Clone)]
pub(crate) struct QueryRevisions {
    /// The most revision in which some input changed.
    pub(crate) changed_at: Revision,

    /// Minimum durability of the inputs to this query.
    pub(crate) durability: Durability,

    /// The inputs that went into our query, if we are tracking them.
    pub(crate) inputs: QueryInputs,
}

/// Every input.
#[derive(Debug, Clone)]
pub(crate) enum QueryInputs {
    /// Non-empty set of inputs, fully known
    Tracked { inputs: Arc<[DatabaseKeyIndex]> },

    /// Empty set of inputs, fully known.
    NoInputs,

    /// Unknown quantity of inputs
    Untracked,
}

impl Default for LocalState {
    fn default() -> Self {
        LocalState {
            query_stack: RefCell::new(Some(Vec::new())),
        }
    }
}

impl LocalState {
    #[inline]
    pub(super) fn push_query(&self, database_key_index: DatabaseKeyIndex) -> ActiveQueryGuard<'_> {
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

    pub(super) fn query_in_progress(&self) -> bool {
        self.with_query_stack(|stack| !stack.is_empty())
    }

    pub(super) fn active_query(&self) -> Option<DatabaseKeyIndex> {
        self.with_query_stack(|stack| {
            stack
                .last()
                .map(|active_query| active_query.database_key_index)
        })
    }

    pub(super) fn report_query_read(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
    ) {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_read(input, durability, changed_at);
            }
        })
    }

    pub(super) fn report_untracked_read(&self, current_revision: Revision) {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_untracked_read(current_revision);
            }
        })
    }

    /// Update the top query on the stack to act as though it read a value
    /// of durability `durability` which changed in `revision`.
    pub(super) fn report_synthetic_read(&self, durability: Durability, revision: Revision) {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_synthetic_read(durability, revision);
            }
        })
    }

    /// Takes the query stack and returns it. This is used when
    /// the current thread is blocking. The stack must be restored
    /// with [`Self::restore_query_stack`] when the thread unblocks.
    pub(super) fn take_query_stack(&self) -> Vec<ActiveQuery> {
        assert!(
            self.query_stack.borrow().is_some(),
            "query stack already taken"
        );
        self.query_stack.take().unwrap()
    }

    /// Restores a query stack taken with [`Self::take_query_stack`] once
    /// the thread unblocks.
    pub(super) fn restore_query_stack(&self, stack: Vec<ActiveQuery>) {
        assert!(self.query_stack.borrow().is_none(), "query stack not taken");
        self.query_stack.replace(Some(stack));
    }
}

impl std::panic::RefUnwindSafe for LocalState {}

/// When a query is pushed onto the `active_query` stack, this guard
/// is returned to represent its slot. The guard can be used to pop
/// the query from the stack -- in the case of unwinding, the guard's
/// destructor will also remove the query.
pub(crate) struct ActiveQueryGuard<'me> {
    local_state: &'me LocalState,
    push_len: usize,
    database_key_index: DatabaseKeyIndex,
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
    pub(super) fn complete(self) -> ActiveQuery {
        let query = self.pop_helper();
        std::mem::forget(self);
        query
    }

    /// As the final action from a pushed query, you can
    /// execute the query implementation. This invokes the
    /// given closure and then returns the "computed query result",
    /// which includes the returned value as well as dependency
    /// and cycle information.
    ///
    /// Executing this method pops the query from the stack.
    #[inline]
    pub(crate) fn pop_and_execute<DB, V>(
        self,
        db: &DB,
        execute: impl FnOnce() -> V,
    ) -> ComputedQueryResult<V>
    where
        DB: ?Sized + Database,
    {
        log::info!("{:?}: executing query", self.database_key_index);

        db.salsa_event(Event {
            runtime_id: db.salsa_runtime().id(),
            kind: EventKind::WillExecute {
                database_key: self.database_key_index,
            },
        });

        // Execute user's code, accumulating inputs etc.
        let value = execute();

        // Extract accumulated inputs.
        let popped_query = self.complete();

        let revisions = popped_query.revisions();

        ComputedQueryResult {
            value,
            revisions,
            cycle_participant: popped_query.cycle,
        }
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.pop_helper();
    }
}
