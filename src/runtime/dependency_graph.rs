use std::mem::{self, ManuallyDrop};
use std::pin::Pin;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::function::SyncGuard;
use crate::key::DatabaseKeyIndex;
use crate::runtime::dependency_graph::edge::EdgeCondvar;
use crate::runtime::WaitResult;
use crate::sync::thread::ThreadId;
use crate::sync::MutexGuard;

#[derive(Debug, Default)]
pub(super) struct DependencyGraph {
    /// A `(K -> V)` pair in this map indicates that the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: FxHashMap<ThreadId, edge::Edge>,

    /// Encodes the `ThreadId` that are blocked waiting for the result
    /// of a given query.
    query_dependents: FxHashMap<DatabaseKeyIndex, SmallVec<[ThreadId; 4]>>,

    /// When a key K completes which had dependent queries Qs blocked on it,
    /// it stores its `WaitResult` here. As they wake up, each query Q in Qs will
    /// come here to fetch their results.
    wait_results: FxHashMap<ThreadId, WaitResult>,
    /// Another thread is blocked on a computation that is happening in a differing thread, having
    /// taken the mutex on this `DependencyGraph`.
    /// This contains the state of that blocked thread.
    ///
    /// Invariant: If this is some then `Runtime::dependency_graph`'s lock is held.
    blocked_on_state: Option<BlockedOnState>,
}

pub struct BlockedOnState {
    pub query_mutex_guard: SyncGuard<'static>,
    pub database_key: DatabaseKeyIndex,
    pub blocked_on: ThreadId,
    pub thread_id: ThreadId,
}

impl std::fmt::Debug for BlockedOnState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockedOnState")
            .field("database_key", &self.database_key)
            .field("other_id", &self.blocked_on)
            .field("thread_id", &self.thread_id)
            .finish()
    }
}

/// A `MutexGuard` on `DependencyGraph` representing that a thread is blocked on a running
/// computation on a different thread.
pub struct Running<'me>(ManuallyDrop<MutexGuard<'me, DependencyGraph>>);

impl DependencyGraph {
    /// True if `from_id` depends on `to_id`.
    ///
    /// (i.e., there is a path from `from_id` to `to_id` in the graph.)
    pub(super) fn depends_on(&self, from_id: ThreadId, to_id: ThreadId) -> bool {
        let mut p = from_id;
        while let Some(q) = self.edges.get(&p).map(|edge| edge.blocked_on_id) {
            if q == to_id {
                return true;
            }

            p = q;
        }
        p == to_id
    }

    /// Modifies the graph so that `from_id` is blocked
    /// on `database_key`, which is being computed by
    /// `to_id`.
    ///
    /// For this to be reasonable, the lock on the
    /// results table for `database_key` must be held.
    /// This ensures that computing `database_key` doesn't
    /// complete before `block_on` executes.
    ///
    /// Preconditions:
    /// * No path from `to_id` to `from_id`
    ///   (i.e., `me.depends_on(to_id, from_id)` is false)
    /// * `held_mutex` is a read lock (or stronger) on `database_key`
    pub(super) fn block_on<QueryMutexGuard>(
        mut me: MutexGuard<'_, Self>,
        from_id: ThreadId,
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
        query_mutex_guard: QueryMutexGuard,
    ) -> WaitResult {
        let cvar = std::pin::pin!(EdgeCondvar::default());
        let cvar = cvar.as_ref();
        // SAFETY: We are blocking until the result is removed from `DependencyGraph::wait_results`
        // at which point the `edge` won't signal the condvar anymore.
        // As such we are keeping the cond var alive until the reference in the edge drops.
        unsafe { me.add_edge(from_id, database_key, to_id, cvar) };

        // Release the mutex that prevents `database_key`
        // from completing, now that the edge has been added.
        drop(query_mutex_guard);

        loop {
            if let Some(result) = me.wait_results.remove(&from_id) {
                debug_assert!(!me.edges.contains_key(&from_id));
                return result;
            }
            me = cvar.wait(me);
        }
    }

    /// Helper for `block_on`: performs actual graph modification
    /// to add a dependency edge from `from_id` to `to_id`, which is
    /// computing `database_key`.
    ///
    /// # Safety
    ///
    /// The caller needs to keep the referent of `cvar` alive until the corresponding
    /// [`Self::wait_results`] entry has been inserted.
    unsafe fn add_edge(
        &mut self,
        from_id: ThreadId,
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
        cvar: Pin<&EdgeCondvar>,
    ) {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));
        debug_assert!(!self.depends_on(to_id, from_id));
        // SAFETY: The caller is responsible for ensuring that the `EdgeGuard` outlives the `Edge`.
        let edge = unsafe { edge::Edge::new(to_id, cvar) };
        self.edges.insert(from_id, edge);
        self.query_dependents
            .entry(database_key)
            .or_default()
            .push(from_id);
    }

    /// Invoked when runtime `to_id` completes executing
    /// `database_key`.
    pub(super) fn unblock_runtimes_blocked_on(
        &mut self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        let dependents = self
            .query_dependents
            .remove(&database_key)
            .unwrap_or_default();

        for from_id in dependents {
            self.unblock_runtime(from_id, wait_result);
        }
    }

    /// Unblock the runtime with the given id with the given wait-result.
    /// This will cause it resume execution (though it will have to grab
    /// the lock on this data structure first, to recover the wait result).
    fn unblock_runtime(&mut self, id: ThreadId, wait_result: WaitResult) {
        let edge = self.edges.remove(&id).expect("not blocked");
        self.wait_results.insert(id, wait_result);

        // Now that we have inserted the `wait_results`,
        // notify the thread.
        edge.notify();
    }
}

mod edge {
    use crate::sync::thread::ThreadId;
    use crate::sync::{Condvar, MutexGuard};

    use std::pin::Pin;

    #[derive(Default, Debug)]
    pub(super) struct EdgeCondvar {
        condvar: Condvar,
        _phantom_pin: std::marker::PhantomPinned,
    }

    impl EdgeCondvar {
        #[inline]
        pub(super) fn wait<'a, T>(&self, mutex_guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
            self.condvar.wait(mutex_guard)
        }
    }

    #[derive(Debug)]
    pub(super) struct Edge {
        pub(super) blocked_on_id: ThreadId,

        /// Signalled whenever a query with dependents completes.
        /// Allows those dependents to check if they are ready to unblock.
        // condvar: unsafe<'stack_frame> Pin<&'stack_frame Condvar>,
        condvar: Pin<&'static EdgeCondvar>,
    }

    impl Edge {
        /// # SAFETY
        ///
        /// The caller must ensure that the [`EdgeCondvar`] is kept alive until the [`Edge`] is dropped.
        pub(super) unsafe fn new(blocked_on_id: ThreadId, condvar: Pin<&EdgeCondvar>) -> Self {
            Self {
                blocked_on_id,
                // SAFETY: The caller is responsible for ensuring that the `EdgeCondvar` outlives the `Edge`.
                condvar: unsafe {
                    std::mem::transmute::<Pin<&EdgeCondvar>, Pin<&'static EdgeCondvar>>(condvar)
                },
            }
        }

        #[inline]
        pub(super) fn notify(self) {
            self.condvar.condvar.notify_one();
        }
    }
}

impl<'me> Running<'me> {
    pub(super) fn new_blocked_on(
        mut me: MutexGuard<'me, DependencyGraph>,
        database_key: DatabaseKeyIndex,
        blocked_on: ThreadId,
        thread_id: ThreadId,
        sync_guard: SyncGuard<'me>,
    ) -> Self {
        debug_assert!(me
            .blocked_on_state
            .replace(BlockedOnState {
                // SAFETY: Self-referential field, we clear `blocked_on_state` on drop or `block_on`
                //call while keeping the `MutexGuard` on `DependencyGraph` alive. As such, the sync
                //guard will never escape the `'me` lifetime.
                query_mutex_guard: unsafe {
                    mem::transmute::<SyncGuard<'me>, SyncGuard<'static>>(sync_guard)
                },
                database_key,
                blocked_on,
                thread_id,
            })
            .is_none());
        Running(ManuallyDrop::new(me))
    }

    #[inline]
    pub(crate) fn database_key(&self) -> DatabaseKeyIndex {
        self.blocked_on_state().database_key
    }

    /// Blocks on the other thread to complete the computation.
    pub(crate) fn block_on(mut self, zalsa: &crate::zalsa::Zalsa) {
        let BlockedOnState {
            query_mutex_guard,
            database_key,
            blocked_on,
            thread_id,
        } = self.take_blocked_on_state();

        zalsa.event(&|| {
            crate::Event::new(crate::EventKind::WillBlockOn {
                other_thread_id: blocked_on,
                database_key,
            })
        });

        crate::tracing::debug!(
            "block_on: thread {thread_id:?} is blocking on {database_key:?} in thread {blocked_on:?}",
        );

        let result = DependencyGraph::block_on(
            // SAFETY: We have ownership, beyond this point we no longer access `self.0`.
            unsafe { ManuallyDrop::take(&mut self.0) },
            thread_id,
            database_key,
            blocked_on,
            query_mutex_guard,
        );

        match result {
            WaitResult::Panicked => {
                // If the other thread panicked, then we consider this thread
                // cancelled. The assumption is that the panic will be detected
                // by the other thread and responded to appropriately.
                crate::Cancelled::PropagatedPanic.throw()
            }
            WaitResult::Completed => {}
        }
    }

    #[inline]
    fn blocked_on_state(&self) -> &BlockedOnState {
        // SAFETY: As per invariant of `blocked_on_state`, this is populated.
        unsafe { self.0.blocked_on_state.as_ref().unwrap_unchecked() }
    }

    #[inline]
    fn take_blocked_on_state(&mut self) -> BlockedOnState {
        // SAFETY: As per invariant of `blocked_on_state`, this is populated.
        unsafe { self.0.blocked_on_state.take().unwrap_unchecked() }
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.take_blocked_on_state();
    }
}

impl std::fmt::Debug for Running<'_> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let BlockedOnState {
            query_mutex_guard: _,
            database_key,
            blocked_on: other_id,
            thread_id,
        } = self.blocked_on_state();

        fmt.debug_struct("Running")
            .field("database_key", database_key)
            .field("other_id", other_id)
            .field("thread_id", thread_id)
            .finish()
    }
}
