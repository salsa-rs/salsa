use std::thread::ThreadId;

use parking_lot::MutexGuard;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::key::DatabaseKeyIndex;
use crate::runtime::WaitResult;

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
}

impl DependencyGraph {
    /// True if `from_id` depends on `to_id`.
    ///
    /// (i.e., there is a path from `from_id` to `to_id` in the graph.)
    pub(super) fn depends_on(&mut self, from_id: ThreadId, to_id: ThreadId) -> bool {
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
        let edge = me.add_edge(from_id, database_key, to_id);

        // Release the mutex that prevents `database_key`
        // from completing, now that the edge has been added.
        drop(query_mutex_guard);

        loop {
            if let Some(result) = me.wait_results.remove(&from_id) {
                debug_assert!(!me.edges.contains_key(&from_id));
                return result;
            }
            edge.wait(&mut me);
        }
    }

    /// Helper for `block_on`: performs actual graph modification
    /// to add a dependency edge from `from_id` to `to_id`, which is
    /// computing `database_key`.
    fn add_edge(
        &mut self,
        from_id: ThreadId,
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
    ) -> edge::EdgeGuard {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));
        debug_assert!(!self.depends_on(to_id, from_id));
        let (edge, guard) = edge::Edge::new(to_id);
        self.edges.insert(from_id, edge);
        self.query_dependents
            .entry(database_key)
            .or_default()
            .push(from_id);
        guard
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
            self.unblock_runtime(from_id, wait_result.clone());
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
    use std::sync::Arc;
    use std::thread::ThreadId;

    use parking_lot::MutexGuard;

    use crate::runtime::dependency_graph::DependencyGraph;

    #[derive(Debug)]
    pub(super) struct Edge {
        pub(super) blocked_on_id: ThreadId,

        /// Signalled whenever a query with dependents completes.
        /// Allows those dependents to check if they are ready to unblock.
        condvar: Arc<parking_lot::Condvar>,
    }

    pub struct EdgeGuard {
        condvar: Arc<parking_lot::Condvar>,
    }

    impl EdgeGuard {
        pub fn wait(&self, mutex_guard: &mut MutexGuard<'_, DependencyGraph>) {
            self.condvar.wait(mutex_guard)
        }
    }

    impl Edge {
        pub(super) fn new(blocked_on_id: ThreadId) -> (Self, EdgeGuard) {
            let condvar = Arc::new(parking_lot::Condvar::new());
            let edge = Self {
                blocked_on_id,
                condvar: condvar.clone(),
            };
            let edge_guard = EdgeGuard { condvar };
            (edge, edge_guard)
        }

        pub(super) fn notify(self) {
            self.condvar.notify_one();
        }
    }
}
