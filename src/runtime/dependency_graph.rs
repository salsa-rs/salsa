use std::pin::Pin;

use rustc_hash::FxHashMap;
use smallvec::{smallvec, SmallVec};

use crate::function::SyncOwnerId;
use crate::key::DatabaseKeyIndex;
use crate::runtime::dependency_graph::edge::EdgeCondvar;
use crate::runtime::WaitResult;
use crate::sync::thread::ThreadId;
use crate::sync::MutexGuard;
use crate::tracing;

#[derive(Debug, Default)]
pub(super) struct DependencyGraph {
    /// A `(K -> V)` pair in this map indicates that the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: Edges,

    /// Encodes the `ThreadId` that are blocked waiting for the result
    /// of a given query.
    query_dependents: FxHashMap<DatabaseKeyIndex, SmallVec<[ThreadId; 4]>>,

    /// When a key K completes which had dependent queries Qs blocked on it,
    /// it stores its `WaitResult` here. As they wake up, each query Q in Qs will
    /// come here to fetch their results.
    wait_results: FxHashMap<ThreadId, WaitResult>,

    /// A `K -> Q` pair indicates that the query `K`'s lock is now owned by the query
    /// `Q`. It's important that `transferred` always forms a tree (must be acyclic),
    /// or else deadlock will result.
    transferred: FxHashMap<DatabaseKeyIndex, (ThreadId, DatabaseKeyIndex)>,

    /// A `K -> [Q]` pair indicates that the query `K` owns the locks of
    /// `Q`. This is the reverse mapping of `transferred` to allow efficient unlocking
    /// of all dependent queries when `K` completes.
    transferred_dependents: FxHashMap<DatabaseKeyIndex, SmallSet<DatabaseKeyIndex, 4>>,
}

impl DependencyGraph {
    /// True if `from_id` depends on `to_id`.
    ///
    /// (i.e., there is a path from `from_id` to `to_id` in the graph.)
    pub(super) fn depends_on(&self, from_id: ThreadId, to_id: ThreadId) -> bool {
        self.edges.depends_on(from_id, to_id)
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

    /// Invoked when the query `database_key` completes and it owns the locks of other queries
    /// (the queries transferred their locks to `database_key`).
    pub(super) fn unblock_runtimes_blocked_on_transferred_queries_owned_by(
        &mut self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        fn unblock_recursive(
            me: &mut DependencyGraph,
            query: DatabaseKeyIndex,
            wait_result: WaitResult,
        ) {
            me.transferred.remove(&query);

            for query in me.transferred_dependents.remove(&query).unwrap_or_default() {
                me.unblock_runtimes_blocked_on(query, wait_result);
                unblock_recursive(me, query, wait_result);
            }
        }

        // If `database_key` is `c` and it has been transferred to `b` earlier, remove its entry.
        tracing::trace!(
            "unblock_runtimes_blocked_on_transferred_queries_owned_by({database_key:?}"
        );

        if let Some((_, owner)) = self.transferred.remove(&database_key) {
            // If this query previously transferred its lock ownership to another query, remove
            // it from that queries dependents as it is now completing.
            self.transferred_dependents
                .get_mut(&owner)
                .unwrap()
                .remove(&database_key);
        }

        unblock_recursive(self, database_key, wait_result);
    }

    pub(super) fn undo_transfer_lock(&mut self, database_key: DatabaseKeyIndex) {
        if let Some((_, owner)) = self.transferred.remove(&database_key) {
            self.transferred_dependents
                .get_mut(&owner)
                .unwrap()
                .remove(&database_key);
        }
    }

    /// Recursively resolves the thread id that currently owns the lock for `database_key`.
    ///
    /// Returns `None` if `database_key` hasn't (or has since then been released) transferred its lock
    /// and the thread id must be looked up in the `SyncTable` instead.
    pub(super) fn thread_id_of_transferred_query(
        &self,
        database_key: DatabaseKeyIndex,
        ignore: Option<DatabaseKeyIndex>,
    ) -> Option<ThreadId> {
        let &(mut resolved_thread, owner) = self.transferred.get(&database_key)?;

        let mut current_owner = owner;

        while let Some(&(next_thread, next_key)) = self.transferred.get(&current_owner) {
            if Some(next_key) == ignore {
                break;
            }
            resolved_thread = next_thread;
            current_owner = next_key;
        }

        Some(resolved_thread)
    }

    /// Modifies the graph so that the lock on `query` (currently owned by `current_thread`) is
    /// transferred to `new_owner` (which is owned by `new_owner_id`).
    pub(super) fn transfer_lock(
        &mut self,
        query: DatabaseKeyIndex,
        current_thread: ThreadId,
        new_owner: DatabaseKeyIndex,
        new_owner_id: SyncOwnerId,
    ) {
        let new_owner_thread = match new_owner_id {
            SyncOwnerId::Thread(thread) => thread,
            SyncOwnerId::Transferred => {
                // Skip over `query` to skip over any existing mapping from `new_owner` to `query` that may
                // exist from previous transfers.
                self.thread_id_of_transferred_query(new_owner, Some(query))
                    .expect("new owner should be blocked on `query`")
            }
        };

        debug_assert!(
            new_owner_thread == current_thread || self.depends_on(new_owner_thread, current_thread),
            "new owner {new_owner:?} ({new_owner_thread:?}) must be blocked on {query:?} ({current_thread:?})"
        );

        let thread_changed = match self.transferred.entry(query) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                // Transfer `c -> b` and there's no existing entry for `c`.
                entry.insert((new_owner_thread, new_owner));
                current_thread != new_owner_thread
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // If we transfer to the same owner as before, return immediately as this is a no-op.
                if entry.get() == &(new_owner_thread, new_owner) {
                    return;
                }

                // `Transfer `c -> b` after a previous `c -> d` mapping.
                // Update the owner and remove the query from the old owner's dependents.
                let &(old_owner_thread, old_owner) = entry.get();

                // For the example below, remove `d` from `b`'s dependents.`
                self.transferred_dependents
                    .get_mut(&old_owner)
                    .unwrap()
                    .remove(&query);

                entry.insert((new_owner_thread, new_owner));

                // If we have `c -> a -> d` and we now insert a mapping `d -> c`, rewrite the mapping to
                // `d -> c -> a` to avoid cycles.
                //
                // Or, starting with `e -> c -> a -> d -> b` insert `d -> c`. We need to respine the tree to
                // ```
                // e -> c -> a -> b
                // d /
                // ```
                //
                //
                // A cycle between transfers can occur when a later iteration has a different outer most query than
                // a previous iteration. The second iteration then hits `cycle_initial` for a different head, (e.g. for `c` where it previously was `d`).
                let mut last_segment = self.transferred.entry(new_owner);

                while let std::collections::hash_map::Entry::Occupied(mut entry) = last_segment {
                    let source = *entry.key();
                    let next_target = entry.get().1;

                    // If it's `a -> d`, remove `a -> d` and insert an edge from `a -> b`
                    if next_target == query {
                        tracing::trace!(
                            "Remap edge {source:?} -> {next_target:?} to {source:?} -> {old_owner:?} to prevent a cycle",
                        );

                        // Remove `a` from the dependents of `d` and remove the mapping from `a -> d`.
                        self.transferred_dependents
                            .get_mut(&query)
                            .unwrap()
                            .remove(&source);

                        // if the old mapping was `c -> d` and we now insert `d -> c`, remove `d -> c`
                        if old_owner == new_owner {
                            entry.remove();
                        } else {
                            // otherwise (when `d` pointed to some other query, e.g. `b` in the example),
                            // add an edge from `a` to `b`
                            entry.insert((old_owner_thread, old_owner));
                            self.transferred_dependents
                                .get_mut(&old_owner)
                                .unwrap()
                                .push(source);
                        }

                        break;
                    }

                    last_segment = self.transferred.entry(next_target);
                }

                // We simply assume here that the thread has changed because we'd have to walk the entire
                // transferred chaine of `old_owner` to know if the thread has changed. This won't save us much
                // compared to just updating all dependent threads.
                true
            }
        };

        // Register `c` as a dependent of `b`.
        let all_dependents = self.transferred_dependents.entry(new_owner).or_default();
        debug_assert!(!all_dependents.contains(&new_owner));
        all_dependents.push(query);

        if thread_changed {
            tracing::debug!("Unblocking new owner of transfer target {new_owner:?}");
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
                    if *id == new_owner_id || self.edges.depends_on(new_owner_id, *id) {
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
        tracing::trace!("update_transferred_edges({query:?}");

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

                tracing::trace!(
                    "Rewrite edge from {:?} to {new_owner_thread:?}",
                    edge.blocked_on_id
                );
                edge.blocked_on_id = new_owner_thread;
                debug_assert!(
                    !&self.edges.depends_on(new_owner_thread, *dependent),
                    "Circular reference between blocked edges: {:#?}",
                    self.edges
                );
            }
        }
    }
}

#[derive(Debug, Default)]
struct Edges(FxHashMap<ThreadId, edge::Edge>);

impl Edges {
    fn depends_on(&self, from_id: ThreadId, to_id: ThreadId) -> bool {
        let mut p = from_id;
        while let Some(q) = self.0.get(&p).map(|edge| edge.blocked_on_id) {
            if q == to_id {
                return true;
            }

            p = q;
        }
        p == to_id
    }

    fn get_mut(&mut self, id: &ThreadId) -> Option<&mut edge::Edge> {
        self.0.get_mut(id)
    }

    fn contains_key(&self, id: &ThreadId) -> bool {
        self.0.contains_key(id)
    }

    fn insert(&mut self, id: ThreadId, edge: edge::Edge) {
        self.0.insert(id, edge);
    }

    fn remove(&mut self, id: &ThreadId) -> Option<edge::Edge> {
        self.0.remove(id)
    }
}

#[derive(Debug)]
struct SmallSet<T, const N: usize>(SmallVec<[T; N]>);

impl<T, const N: usize> SmallSet<T, N>
where
    T: PartialEq,
{
    const fn new() -> Self {
        Self(SmallVec::new_const())
    }

    fn push(&mut self, value: T) {
        debug_assert!(!self.0.contains(&value));

        self.0.push(value);
    }

    fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }

    fn remove(&mut self, value: &T) -> bool {
        if let Some(index) = self.0.iter().position(|x| x == value) {
            self.0.swap_remove(index);
            true
        } else {
            false
        }
    }

    fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }
}

impl<T, const N: usize> IntoIterator for SmallSet<T, N> {
    type Item = T;
    type IntoIter = smallvec::IntoIter<[T; N]>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a SmallSet<T, N>
where
    T: PartialEq,
{
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T, const N: usize> Default for SmallSet<T, N>
where
    T: PartialEq,
{
    fn default() -> Self {
        Self::new()
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
