use crate::durability::Durability;
use crate::runtime::ActiveQuery;
use crate::runtime::Revision;
use crate::DatabaseKeyIndex;
use std::cell::RefCell;

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

impl Default for LocalState {
    fn default() -> Self {
        LocalState {
            query_stack: RefCell::new(Some(Vec::new())),
        }
    }
}

impl LocalState {
    pub(super) fn push_query(
        &self,
        database_key_index: DatabaseKeyIndex,
        max_durability: Durability,
    ) -> ActiveQueryGuard<'_> {
        let mut query_stack = self.query_stack.borrow_mut();
        let query_stack = query_stack.as_mut().expect("local stack taken");
        query_stack.push(ActiveQuery::new(database_key_index, max_durability));
        ActiveQueryGuard {
            local_state: self,
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

    pub(super) fn report_synthetic_read(&self, durability: Durability, current_revision: Revision) {
        self.with_query_stack(|stack| {
            if let Some(top_query) = stack.last_mut() {
                top_query.add_synthetic_read(durability, current_revision);
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
        self.query_stack.replace(None).unwrap()
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
pub(super) struct ActiveQueryGuard<'me> {
    local_state: &'me LocalState,
    push_len: usize,
}

impl ActiveQueryGuard<'_> {
    fn pop_helper(&self) -> ActiveQuery {
        self.local_state.with_query_stack(|stack| {
            // Sanity check: pushes and pops should be balanced.
            assert_eq!(stack.len(), self.push_len);

            stack.pop().unwrap()
        })
    }

    /// Invoked when the query has successfully completed execution.
    pub(super) fn complete(self) -> ActiveQuery {
        let query = self.pop_helper();
        std::mem::forget(self);
        query
    }
}

impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.pop_helper();
    }
}
