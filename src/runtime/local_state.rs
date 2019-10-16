use crate::dependency::Dependency;
use crate::durability::Durability;
use crate::runtime::ActiveQuery;
use crate::runtime::Revision;
use crate::Database;
use std::cell::{Ref, RefCell, RefMut};

/// State that is specific to a single execution thread.
///
/// Internally, this type uses ref-cells.
///
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
pub(super) struct LocalState<DB: Database> {
    /// Vector of active queries.
    ///
    /// Unwinding note: pushes onto this vector must be popped -- even
    /// during unwinding.
    query_stack: RefCell<Vec<ActiveQuery<DB>>>,
}

impl<DB: Database> Default for LocalState<DB> {
    fn default() -> Self {
        LocalState {
            query_stack: Default::default(),
        }
    }
}

impl<DB: Database> LocalState<DB> {
    pub(super) fn push_query<'db>(
        db: &'db mut DB,
        database_key: &DB::DatabaseKey,
        max_durability: Durability,
    ) -> ActiveQueryGuard<'db, DB> {
        let push_len = {
            let mut query_stack = db.salsa_runtime().local_state.query_stack.borrow_mut();
            query_stack.push(ActiveQuery::new(database_key.clone(), max_durability));
            query_stack.len()
        };
        ActiveQueryGuard { db, push_len }
    }

    /// Returns a reference to the active query stack.
    ///
    /// **Warning:** Because this reference holds the ref-cell lock,
    /// you should not use any mutating methods of `LocalState` while
    /// reading from it.
    pub(super) fn borrow_query_stack(&self) -> Ref<'_, Vec<ActiveQuery<DB>>> {
        self.query_stack.borrow()
    }

    pub(super) fn borrow_query_stack_mut(&self) -> RefMut<'_, Vec<ActiveQuery<DB>>> {
        self.query_stack.borrow_mut()
    }

    pub(super) fn query_in_progress(&self) -> bool {
        !self.query_stack.borrow().is_empty()
    }

    pub(super) fn active_query(&self) -> Option<DB::DatabaseKey> {
        self.query_stack
            .borrow()
            .last()
            .map(|active_query| active_query.database_key.clone())
    }

    pub(super) fn report_query_read(
        &self,
        dependency: Dependency<DB>,
        durability: Durability,
        changed_at: Revision,
    ) {
        if let Some(top_query) = self.query_stack.borrow_mut().last_mut() {
            top_query.add_read(dependency, durability, changed_at);
        }
    }

    pub(super) fn report_untracked_read(&self, current_revision: Revision) {
        if let Some(top_query) = self.query_stack.borrow_mut().last_mut() {
            top_query.add_untracked_read(current_revision);
        }
    }

    pub(super) fn report_synthetic_read(&self, durability: Durability) {
        if let Some(top_query) = self.query_stack.borrow_mut().last_mut() {
            top_query.add_synthetic_read(durability);
        }
    }

    pub(super) fn report_anon_read(&self, revision: Revision) {
        if let Some(top_query) = self.query_stack.borrow_mut().last_mut() {
            top_query.add_anon_read(revision);
        }
    }
}

impl<DB> std::panic::RefUnwindSafe for LocalState<DB> where DB: Database {}

/// When a query is pushed onto the `active_query` stack, this guard
/// is returned to represent its slot. The guard can be used to pop
/// the query from the stack -- in the case of unwinding, the guard's
/// destructor will also remove the query.
pub(super) struct ActiveQueryGuard<'me, DB: Database> {
    pub(super) db: &'me mut DB,
    push_len: usize,
}

impl<'me, DB> ActiveQueryGuard<'me, DB>
where
    DB: Database,
{
    fn pop_helper(&self) -> ActiveQuery<DB> {
        let mut query_stack = self.db.salsa_runtime().local_state.query_stack.borrow_mut();

        // Sanity check: pushes and pops should be balanced.
        assert_eq!(query_stack.len(), self.push_len);

        query_stack.pop().unwrap()
    }

    /// Invoked when the query has successfully completed execution.
    pub(super) fn complete(self) -> ActiveQuery<DB> {
        let query = self.pop_helper();
        std::mem::forget(self);
        query
    }
}

impl<'me, DB> Drop for ActiveQueryGuard<'me, DB>
where
    DB: Database,
{
    fn drop(&mut self) {
        self.pop_helper();
    }
}
