use self::dependency_graph::{CanClaimTransferred, DependencyGraph};
use crate::durability::Durability;
use crate::function::{SyncGuard, SyncOwnerId, SyncState};
use crate::key::DatabaseKeyIndex;
use crate::sync::atomic::{AtomicBool, Ordering};
use crate::sync::thread::{self, ThreadId};
use crate::sync::Mutex;
use crate::table::Table;
use crate::zalsa::Zalsa;
use crate::{Cancelled, Event, EventKind, Id, Revision};

mod dependency_graph;

#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
pub struct Runtime {
    /// Set to true when the current revision has been canceled.
    /// This is done when we an input is being changed. The flag
    /// is set back to false once the input has been changed.
    #[cfg_attr(feature = "persistence", serde(skip))]
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
    #[cfg_attr(feature = "persistence", serde(skip))]
    dependency_graph: Mutex<DependencyGraph>,

    /// Data for instances
    #[cfg_attr(feature = "persistence", serde(skip))]
    table: Table,
}

#[derive(Copy, Clone, Debug)]
pub(super) enum WaitResult {
    Completed,
    Panicked,
}

#[derive(Debug)]
pub(crate) enum BlockResult<'me> {
    /// The query is running on another thread.
    Running(Running<'me>),

    /// Blocking resulted in a cycle.
    ///
    /// The lock is hold by the current thread or there's another thread that is waiting on the current thread,
    /// and blocking this thread on the other thread would result in a deadlock/cycle.
    Cycle,
}

pub(crate) enum BlockTransferredResult<'me> {
    /// The current thread is the owner of the transferred query
    /// and it can claim it if it wants to.
    ImTheOwner,

    /// The query is owned/running on another thread.
    OwnedBy(Box<BlockOnTransferredOwner<'me>>),

    /// The query has transferred its ownership to another query previously but that query has
    /// since then completed and released the lock.
    Released,
}

pub(super) struct BlockOnTransferredOwner<'me> {
    dg: crate::sync::MutexGuard<'me, DependencyGraph>,
    /// The query that we're trying to claim.
    database_key: DatabaseKeyIndex,
    /// The thread that currently owns the lock for the transferred query.
    other_id: ThreadId,
    /// The current thread that is trying to claim the transferred query.
    thread_id: ThreadId,
}

impl<'me> BlockOnTransferredOwner<'me> {
    /// Block on the other thread to complete the computation.
    pub(super) fn block(self, query_mutex_guard: SyncGuard<'me>) -> BlockResult<'me> {
        // Cycle in the same thread.
        if self.thread_id == self.other_id {
            return BlockResult::Cycle;
        }

        if self.dg.depends_on(self.other_id, self.thread_id) {
            crate::tracing::debug!(
                "block_on: cycle detected for {:?} in thread {thread_id:?} on {:?}",
                self.database_key,
                self.other_id,
                thread_id = self.thread_id
            );
            return BlockResult::Cycle;
        }

        BlockResult::Running(Running(Box::new(BlockedOnInner {
            dg: self.dg,
            query_mutex_guard,
            database_key: self.database_key,
            other_id: self.other_id,
            thread_id: self.thread_id,
        })))
    }
}

pub struct Running<'me>(Box<BlockedOnInner<'me>>);

struct BlockedOnInner<'me> {
    dg: crate::sync::MutexGuard<'me, DependencyGraph>,
    query_mutex_guard: SyncGuard<'me>,
    database_key: DatabaseKeyIndex,
    other_id: ThreadId,
    thread_id: ThreadId,
}

impl Running<'_> {
    /// Blocks on the other thread to complete the computation.
    pub(crate) fn block_on(self, zalsa: &Zalsa) {
        let BlockedOnInner {
            dg,
            query_mutex_guard,
            database_key,
            other_id,
            thread_id,
        } = *self.0;

        zalsa.event(&|| {
            Event::new(EventKind::WillBlockOn {
                other_thread_id: other_id,
                database_key,
            })
        });

        crate::tracing::info!(
            "block_on: thread {thread_id:?} is blocking on {database_key:?} in thread {other_id:?}",
        );

        let result =
            DependencyGraph::block_on(dg, thread_id, database_key, other_id, query_mutex_guard);

        match result {
            WaitResult::Panicked => {
                // If the other thread panicked, then we consider this thread
                // cancelled. The assumption is that the panic will be detected
                // by the other thread and responded to appropriately.
                Cancelled::PropagatedPanic.throw()
            }
            WaitResult::Completed => {}
        }
    }
}

impl std::fmt::Debug for Running<'_> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Running")
            .field("database_key", &self.0.database_key)
            .field("other_id", &self.0.other_id)
            .field("thread_id", &self.0.thread_id)
            .finish()
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Stamp {
    pub durability: Durability,
    pub changed_at: Revision,
}

pub fn stamp(revision: Revision, durability: Durability) -> Stamp {
    Stamp {
        durability,
        changed_at: revision,
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
    #[inline]
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
        crate::tracing::trace!("set_cancellation_flag");
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
        crate::tracing::info!("new_revision: {r_old:?} -> {r_new:?}");
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
    pub(crate) fn block<'a>(
        &'a self,
        database_key: DatabaseKeyIndex,
        other_id: ThreadId,
        query_mutex_guard: SyncGuard<'a>,
    ) -> BlockResult<'a> {
        let thread_id = thread::current().id();
        // Cycle in the same thread.
        if thread_id == other_id {
            return BlockResult::Cycle;
        }

        let dg = self.dependency_graph.lock();

        if dg.depends_on(other_id, thread_id) {
            crate::tracing::debug!("block_on: cycle detected for {database_key:?} in thread {thread_id:?} on {other_id:?}");
            return BlockResult::Cycle;
        }

        BlockResult::Running(Running(Box::new(BlockedOnInner {
            dg,
            query_mutex_guard,
            database_key,
            other_id,
            thread_id,
        })))
    }

    /// Tries to claim ownership of a transferred query where `thread_id` is the current thread and `query`
    /// is the query (that had its ownership transferred) to claim.
    ///
    /// For this operation to be reasonable, the caller must ensure that the lock on `query` is not released
    /// before this operation completes.
    pub(super) fn block_transferred(
        &self,
        query: DatabaseKeyIndex,
        current_id: ThreadId,
    ) -> BlockTransferredResult<'_> {
        let mut dg = self.dependency_graph.lock();

        let owner_thread = dg.resolved_transferred_id(query, None);

        let Some(owner_thread_id) = owner_thread else {
            // The query transferred its ownership but the owner has since then released the lock.
            return BlockTransferredResult::Released;
        };

        if owner_thread_id == current_id || dg.depends_on(owner_thread_id, current_id) {
            BlockTransferredResult::ImTheOwner
        } else {
            // Lock is owned by another thread, wait for it to be released.
            BlockTransferredResult::OwnedBy(Box::new(BlockOnTransferredOwner {
                dg,
                database_key: query,
                other_id: owner_thread_id,
                thread_id: current_id,
            }))
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

    #[cold]
    pub(crate) fn unblock_transferred_queries(
        &self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        self.dependency_graph
            .lock()
            .unblock_transferred_queries(database_key, wait_result);
    }

    #[cold]
    pub(super) fn remove_transferred(&self, query: DatabaseKeyIndex) {
        self.dependency_graph.lock().remove_transferred(query);
    }

    pub(super) fn transfer_lock(
        &self,
        query: DatabaseKeyIndex,
        current_thread: ThreadId,
        new_owner: DatabaseKeyIndex,
        new_owner_thread: SyncOwnerId,
    ) {
        self.dependency_graph.lock().transfer_lock(
            query,
            current_thread,
            new_owner,
            new_owner_thread,
        );
    }

    #[cfg(feature = "persistence")]
    pub(crate) fn deserialize_from(&mut self, other: &mut Runtime) {
        // The only field that is serialized is `revisions`.
        self.revisions = other.revisions;
    }
}
