use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

use parking_lot::Mutex;

use self::dependency_graph::DependencyGraph;
use crate::durability::Durability;
use crate::key::DatabaseKeyIndex;
use crate::table::Table;
use crate::{Cancelled, Database, Event, EventKind, Revision};

mod dependency_graph;

pub struct Runtime {
    /// Set to true when the current revision has been canceled.
    /// This is done when we an input is being changed. The flag
    /// is set back to false once the input has been changed.
    revision_canceled: AtomicBool,

    /// Stores the "last change" revision for values of each duration.
    /// This vector is always of length at least 1 (for Durability 0)
    /// but its total length depends on the number of durations. The
    /// element at index 0 is special as it represents the "current
    /// revision".  In general, we have the invariant that revisions
    /// in here are *declining* -- that is, `revisions[i] >=
    /// revisions[i + 1]`, for all `i`. This is because when you
    /// modify a value with durability D, that implies that values
    /// with durability less than D may have changed too.
    revisions: [Revision; Durability::LEN],

    /// The dependency graph tracks which runtimes are blocked on one
    /// another, waiting for queries to terminate.
    dependency_graph: Mutex<DependencyGraph>,

    /// Data for instances
    table: Table,
}

#[derive(Clone, Debug)]
pub(crate) enum WaitResult {
    Completed,
    Panicked,
}

#[derive(Clone, Debug)]
pub(crate) enum BlockResult {
    Completed,
    Cycle,
}

#[derive(Copy, Clone, Debug)]
pub struct StampedValue<V> {
    pub value: V,
    pub durability: Durability,
    pub changed_at: Revision,
}

pub type Stamp = StampedValue<()>;

pub fn stamp(revision: Revision, durability: Durability) -> Stamp {
    StampedValue {
        value: (),
        durability,
        changed_at: revision,
    }
}

impl<V> StampedValue<V> {
    // FIXME: Use or remove this.
    #[allow(dead_code)]
    pub(crate) fn merge_revision_info<U>(&mut self, other: &StampedValue<U>) {
        self.durability = self.durability.min(other.durability);
        self.changed_at = self.changed_at.max(other.changed_at);
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            revisions: [Revision::start(); Durability::LEN],
            revision_canceled: Default::default(),
            dependency_graph: Default::default(),
            table: Default::default(),
        }
    }
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Runtime")
            .field("revisions", &self.revisions)
            .field("revision_canceled", &self.revision_canceled)
            .field("dependency_graph", &self.dependency_graph)
            .finish()
    }
}

impl Runtime {
    pub(crate) fn current_revision(&self) -> Revision {
        self.revisions[0]
    }

    /// Reports that an input with durability `durability` changed.
    /// This will update the 'last changed at' values for every durability
    /// less than or equal to `durability` to the current revision.
    pub(crate) fn report_tracked_write(&mut self, durability: Durability) {
        let new_revision = self.current_revision();
        self.revisions[1..=durability.index()].fill(new_revision);
    }

    /// The revision in which values with durability `d` may have last
    /// changed.  For D0, this is just the current revision. But for
    /// higher levels of durability, this value may lag behind the
    /// current revision. If we encounter a value of durability Di,
    /// then, we can check this function to get a "bound" on when the
    /// value may have changed, which allows us to skip walking its
    /// dependencies.
    #[inline]
    pub(crate) fn last_changed_revision(&self, d: Durability) -> Revision {
        self.revisions[d.index()]
    }

    pub(crate) fn load_cancellation_flag(&self) -> bool {
        self.revision_canceled.load(Ordering::Acquire)
    }

    pub(crate) fn set_cancellation_flag(&self) {
        self.revision_canceled.store(true, Ordering::Release);
    }

    pub(crate) fn reset_cancellation_flag(&mut self) {
        *self.revision_canceled.get_mut() = false;
    }

    /// Returns the [`Table`] used to store the value of salsa structs
    #[inline]
    pub(crate) fn table(&self) -> &Table {
        &self.table
    }

    pub(crate) fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    /// Increments the "current revision" counter and clears
    /// the cancellation flag.
    ///
    /// This should only be done by the storage when the state is "quiescent".
    pub(crate) fn new_revision(&mut self) -> Revision {
        let r_old = self.current_revision();
        let r_new = r_old.next();
        self.revisions[0] = r_new;
        r_new
    }

    /// Block until `other_id` completes executing `database_key`, or return `BlockResult::Cycle`
    /// immediately in case of a cycle.
    ///
    /// `query_mutex_guard` is the guard for the current query's state;
    /// it will be dropped after we have successfully registered the
    /// dependency.
    ///
    /// # Propagating panics
    ///
    /// If the thread `other_id` panics, then our thread is considered
    /// cancelled, so this function will panic with a `Cancelled` value.
    pub(crate) fn block_on<QueryMutexGuard>(
        &self,
        db: &(impl Database + ?Sized),
        database_key: DatabaseKeyIndex,
        other_id: ThreadId,
        query_mutex_guard: QueryMutexGuard,
    ) -> BlockResult {
        let mut dg = self.dependency_graph.lock();
        let thread_id = std::thread::current().id();

        if dg.depends_on(other_id, thread_id) {
            return BlockResult::Cycle;
        }

        db.salsa_event(&|| {
            Event::new(EventKind::WillBlockOn {
                other_thread_id: other_id,
                database_key,
            })
        });

        let result =
            DependencyGraph::block_on(dg, thread_id, database_key, other_id, query_mutex_guard);

        match result {
            WaitResult::Completed => BlockResult::Completed,

            // If the other thread panicked, then we consider this thread
            // cancelled. The assumption is that the panic will be detected
            // by the other thread and responded to appropriately.
            WaitResult::Panicked => Cancelled::PropagatedPanic.throw(),
        }
    }

    /// Invoked when this runtime completed computing `database_key` with
    /// the given result `wait_result` (`wait_result` should be `None` if
    /// computing `database_key` panicked and could not complete).
    /// This function unblocks any dependent queries and allows them
    /// to continue executing.
    pub(crate) fn unblock_queries_blocked_on(
        &self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        self.dependency_graph
            .lock()
            .unblock_runtimes_blocked_on(database_key, wait_result);
    }
}
