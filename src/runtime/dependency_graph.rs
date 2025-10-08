use rustc_hash::FxHashMap;
use smallvec::{smallvec, SmallVec};
use std::pin::Pin;

use crate::function::SyncOwnerId;
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

    /// A `K -> Q` pair indicates that `K`'s lock is now owned by
    /// `Q` (The thread id of `Q` and its database key)
    transferred: FxHashMap<DatabaseKeyIndex, (ThreadId, DatabaseKeyIndex)>,

    /// A `K -> Qs` pair indicates that `K`'s lock is now owned by
    /// `Qs` (The thread id of `Qs` and their database keys)
    transferred_dependents: FxHashMap<DatabaseKeyIndex, SmallVec<[DatabaseKeyIndex; 4]>>,
}

impl DependencyGraph {
    /// True if `from_id` depends on `to_id`.
    ///
    /// (i.e., there is a path from `from_id` to `to_id` in the graph.)
    pub(super) fn depends_on(&self, from_id: ThreadId, to_id: ThreadId) -> bool {
        Self::depends_on_impl(&self.edges, from_id, to_id)
    }

    fn depends_on_impl(
        edges: &FxHashMap<ThreadId, edge::Edge>,
        from_id: ThreadId,
        to_id: ThreadId,
    ) -> bool {
        let mut p = from_id;
        while let Some(q) = edges.get(&p).map(|edge| edge.blocked_on_id) {
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
        tracing::debug!(
            "Unblocking runtimes blocked on {database_key:?} with wait result {wait_result:?}"
        );
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
        tracing::debug!("Unblocking runtime {id:?} with wait result {wait_result:?}");
        let edge = self.edges.remove(&id).expect("not blocked");
        self.wait_results.insert(id, wait_result);

        // Now that we have inserted the `wait_results`,
        // notify the thread.
        edge.notify();
    }

    pub(super) fn unblock_transferred_queries(
        &mut self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        // If `database_key` is `c` and it has been transfered to `b` earlier, remove its entry.
        tracing::debug!("unblock_transferred_queries({database_key:?}");
        if let Some((_, owner)) = self.transferred.remove(&database_key) {
            let owner_dependents = self.transferred_dependents.get_mut(&owner).unwrap();
            let index = owner_dependents
                .iter()
                .position(|&x| x == database_key)
                .unwrap();
            owner_dependents.swap_remove(index);
        }

        let mut unblocked: SmallVec<[_; 4]> = SmallVec::new();
        let mut queue: SmallVec<[_; 4]> = smallvec![database_key];

        while let Some(current) = queue.pop() {
            self.transferred.remove(&current);
            let transitive = self
                .transferred_dependents
                .remove(&current)
                .unwrap_or_default();

            queue.extend(transitive);

            unblocked.push(current);
        }

        for query in unblocked {
            self.unblock_runtimes_blocked_on(query, wait_result);
        }
    }

    /// Returns `Ok(thread_id)` if `database_key_index` is a query who's lock ownership has been transferred to `thread_id` (potentially over multiple steps)
    /// and the lock was claimed. Returns `Err(Some(thread_id))` if the lock was not claimed.
    ///
    /// Returns `Err(None)` if `database_key_index` hasn't been transferred or its owning lock has since then been removed.
    pub(super) fn block_on_transferred(
        &mut self,
        database_key_index: DatabaseKeyIndex,
        current_id: ThreadId,
    ) -> Result<DatabaseKeyIndex, Option<ThreadId>> {
        let owner_thread = self.resolved_transferred_id(database_key_index, None);

        let Some((thread_id, owner_key)) = owner_thread else {
            return Err(None);
        };

        if thread_id == current_id || self.depends_on(thread_id, current_id) {
            Ok(owner_key)
        } else {
            Err(Some(thread_id))
        }
    }

    pub(super) fn remove_transferred(&mut self, database_key: DatabaseKeyIndex) {
        if let Some((_, owner)) = self.transferred.remove(&database_key) {
            let dependents = self.transferred_dependents.get_mut(&owner).unwrap();
            let index = dependents.iter().position(|h| *h == database_key).unwrap();
            dependents.swap_remove(index);
        }
    }

    pub(super) fn resolved_transferred_id(
        &self,
        database_key: DatabaseKeyIndex,
        ignore: Option<DatabaseKeyIndex>,
    ) -> Option<(ThreadId, DatabaseKeyIndex)> {
        let &(mut resolved_thread, owner) = self.transferred.get(&database_key)?;

        let mut current_owner = owner;

        while let Some(&(next_thread, next_key)) = self.transferred.get(&current_owner) {
            if Some(next_key) == ignore {
                break;
            }
            resolved_thread = next_thread;
            current_owner = next_key;
        }

        Some((resolved_thread, owner))
    }

    pub(super) fn transfer_lock(
        &mut self,
        query: DatabaseKeyIndex,
        current_thread: ThreadId,
        new_owner: DatabaseKeyIndex,
        new_owner_thread: SyncOwnerId,
    ) {
        let new_owner_thread = match new_owner_thread {
            SyncOwnerId::Thread(thread) => thread,
            SyncOwnerId::Transferred => {
                self.resolved_transferred_id(new_owner, Some(query))
                    .unwrap()
                    .0
            }
        };

        let mut owner_changed = current_thread != new_owner_thread;

        // TODO: Can we move this into the occupied branch? It's pointless to run this check if there's no existing mapping.

        // If we have `c -> a -> d` and we now insert a mapping `d -> c`, rewrite the mapping to
        // `d -> c -> a` to avoid cycles.
        //
        // A more complex is  `e -> c -> a -> d -> b` where we now transfer `d -> c`. Respine
        // ```
        // e -> c -> a -> b
        // d /
        // ```
        //
        // The first part here only takes care of removing `d` form ` a -> d -> b` (so that it becomes `a -> b`).
        // The `d -> c` mapping is inserted by the `match` statement below.
        //
        // A cycle between transfers can occur when a later iteration has a different outer most query than
        // a previous iteration. The second iteration then hits `cycle_initial` for a different head, (e.g. for `c` where it previously was `d`).
        let mut last_segment = self.transferred.entry(new_owner);

        while let std::collections::hash_map::Entry::Occupied(entry) = last_segment {
            let next_target = entry.get().1;
            if next_target == query {
                tracing::debug!(
                    "Remove mapping from {:?} to {:?} to prevent a cycle",
                    entry.key(),
                    query
                );

                // Remove `b` from the dependents of `d` and remove the mapping from `a -> d`.
                let old_dependents = self.transferred_dependents.get_mut(&query).unwrap();
                let index = old_dependents
                    .iter()
                    .position(|key| key == entry.key())
                    .unwrap();
                old_dependents.swap_remove(index);
                // `a` in `a -> d`
                let previous_source = *entry.key();
                entry.remove();

                // If there's a `d -> b` mapping, remove `d` from `b`'s dependents and connect `a` with `b`
                if let Some(next_next) = self.transferred.remove(&query) {
                    // connect `a` with `b` (okay to use `insert` because we removed the `a` mapping before).
                    self.transferred.insert(previous_source, next_next);
                    let next_next_dependents =
                        self.transferred_dependents.get_mut(&next_next.1).unwrap();
                    let query_index = next_next_dependents
                        .iter()
                        .position(|key| *key == query)
                        .unwrap();
                    next_next_dependents[query_index] = previous_source;
                }

                break;
            }

            last_segment = self.transferred.entry(next_target);
        }

        // TODO: Skip unblocks for transitive queries if the old owner is the same as the new owner?
        match self.transferred.entry(query) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                // Transfer `c -> b` and there's no existing entry for `c`.
                entry.insert((new_owner_thread, new_owner));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get() == &(new_owner_thread, new_owner) {
                    return;
                }

                // `Transfer `c -> b` after a previous `c -> d` mapping.
                // Update the owner and remove the query from the old owner's dependents.
                let old_owner = entry.get().1;

                owner_changed = true;
                let old_dependents = self.transferred_dependents.get_mut(&old_owner).unwrap();
                let index = old_dependents.iter().position(|key| *key == query).unwrap();
                old_dependents.swap_remove(index);

                entry.insert((new_owner_thread, new_owner));
            }
        };

        // Register `c` as a dependent of `b`.
        let all_dependents = self.transferred_dependents.entry(new_owner).or_default();
        assert!(!all_dependents.contains(&query));
        assert!(!all_dependents.contains(&new_owner));
        all_dependents.push(query);

        tracing::debug!("Wake up blocked threads after transferring ownership to {new_owner:?}");
        if owner_changed {
            self.unblock_transfer_target(query, new_owner_thread);
            self.update_transferred_edges(query, new_owner_thread);
        }
    }

    /// Finds the one query in the dependents of the `source_query` (the one that is transferred to a new owner)
    /// on which the `new_owner_id` thread blocks on and unblocks it, to ensure progress.
    fn unblock_transfer_target(&mut self, source_query: DatabaseKeyIndex, new_owner_id: ThreadId) {
        let mut queue: SmallVec<[_; 4]> = smallvec![source_query];

        while let Some(current) = queue.pop() {
            if let Some(dependents) = self.query_dependents.get_mut(&current) {
                for (i, id) in dependents.iter().enumerate() {
                    if *id == new_owner_id || Self::depends_on_impl(&self.edges, new_owner_id, *id)
                    {
                        let thread_id = dependents.swap_remove(i);
                        if dependents.is_empty() {
                            self.query_dependents.remove(&current);
                        }

                        self.unblock_runtime(thread_id, WaitResult::Completed);

                        return;
                    }
                }
            };

            queue.extend(
                self.transferred_dependents
                    .get(&current)
                    .iter()
                    .copied()
                    .flatten()
                    .copied(),
            );
        }
    }

    fn update_transferred_edges(&mut self, query: DatabaseKeyIndex, new_owner_thread: ThreadId) {
        tracing::info!("Resuming transitive dependents of query {query:?}");

        let mut queue: SmallVec<[_; 4]> = smallvec![query];

        while let Some(query) = queue.pop() {
            queue.extend(
                self.transferred_dependents
                    .get(&query)
                    .iter()
                    .copied()
                    .flatten()
                    .copied(),
            );

            let Some(dependents) = self.query_dependents.get_mut(&query) else {
                continue;
            };

            for dependent in dependents.iter_mut() {
                let edge = self.edges.get_mut(dependent).unwrap();

                tracing::info!(
                    "Rewrite edge from {:?} to {new_owner_thread:?}",
                    edge.blocked_on_id
                );
                edge.blocked_on_id = new_owner_thread;
            }

            #[cfg(debug_assertions)]
            {
                for id in self.query_dependents.get(&query).into_iter().flatten() {
                    debug_assert!(
                        !self.depends_on(new_owner_thread, *id),
                        "Circular reference between blocked edges: {:#?}",
                        self.edges
                    );
                }
            }
        }
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
