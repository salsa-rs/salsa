use std::thread::ThreadId;

use crate::active_query::ActiveQuery;
use crate::key::DatabaseKeyIndex;
use crate::runtime::WaitResult;
use parking_lot::MutexGuard;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

#[derive(Debug, Default)]
pub(super) struct DependencyGraph {
    /// A `(K -> V)` pair in this map indicates that the the runtime
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

    /// Invokes `closure` with a `&mut ActiveQuery` for each query that participates in the cycle.
    /// The cycle runs as follows:
    ///
    /// 1. The runtime `from_id`, which has the stack `from_stack`, would like to invoke `database_key`...
    /// 2. ...but `database_key` is already being executed by `to_id`...
    /// 3. ...and `to_id` is transitively dependent on something which is present on `from_stack`.
    pub(super) fn for_each_cycle_participant(
        &mut self,
        from_id: ThreadId,
        from_stack: &mut [ActiveQuery],
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
        mut closure: impl FnMut(&mut [ActiveQuery]),
    ) {
        debug_assert!(self.depends_on(to_id, from_id));

        // To understand this algorithm, consider this [drawing](https://is.gd/TGLI9v):
        //
        //    database_key = QB2
        //    from_id = A
        //    to_id = B
        //    from_stack = [QA1, QA2, QA3]
        //
        //    self.edges[B] = { C, QC2, [QB1..QB3] }
        //    self.edges[C] = { A, QA2, [QC1..QC3] }
        //
        //         The cyclic
        //         edge we have
        //         failed to add.
        //           :
        //    A      :    B         C
        //           :
        //    QA1    v    QB1       QC1
        // ┌► QA2    ┌──► QB2   ┌─► QC2
        // │  QA3 ───┘    QB3 ──┘   QC3 ───┐
        // │                               │
        // └───────────────────────────────┘
        //
        // Final output: [QB2, QB3, QC2, QC3, QA2, QA3]

        let mut id = to_id;
        let mut key = database_key;
        while id != from_id {
            // Looking at the diagram above, the idea is to
            // take the edge from `to_id` starting at `key`
            // (inclusive) and down to the end. We can then
            // load up the next thread (i.e., we start at B/QB2,
            // and then load up the dependency on C/QC2).
            let edge = self.edges.get_mut(&id).unwrap();
            closure(strip_prefix_query_stack_mut(edge.stack_mut(), key));
            id = edge.blocked_on_id;
            key = edge.blocked_on_key;
        }

        // Finally, we copy in the results from `from_stack`.
        closure(strip_prefix_query_stack_mut(from_stack, key));
    }

    /// Unblock each blocked runtime (excluding the current one) if some
    /// query executing in that runtime is participating in cycle fallback.
    ///
    /// Returns a boolean (Current, Others) where:
    /// * Current is true if the current runtime has cycle participants
    ///   with fallback;
    /// * Others is true if other runtimes were unblocked.
    pub(super) fn maybe_unblock_runtimes_in_cycle(
        &mut self,
        from_id: ThreadId,
        from_stack: &[ActiveQuery],
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
    ) -> (bool, bool) {
        // See diagram in `for_each_cycle_participant`.
        let mut id = to_id;
        let mut key = database_key;
        let mut others_unblocked = false;
        while id != from_id {
            let edge = self.edges.get(&id).unwrap();
            let next_id = edge.blocked_on_id;
            let next_key = edge.blocked_on_key;

            if let Some(cycle) = strip_prefix_query_stack(edge.stack(), key)
                .iter()
                .rev()
                .find_map(|aq| aq.cycle.clone())
            {
                // Remove `id` from the list of runtimes blocked on `next_key`:
                self.query_dependents
                    .get_mut(&next_key)
                    .unwrap()
                    .retain(|r| *r != id);

                // Unblock runtime so that it can resume execution once lock is released:
                self.unblock_runtime(id, WaitResult::Cycle(cycle));

                others_unblocked = true;
            }

            id = next_id;
            key = next_key;
        }

        let this_unblocked = strip_prefix_query_stack(from_stack, key)
            .iter()
            .any(|aq| aq.cycle.is_some());

        (this_unblocked, others_unblocked)
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
        from_stack: &mut [ActiveQuery],
        query_mutex_guard: QueryMutexGuard,
    ) -> WaitResult {
        // SAFETY: We are blocking until the result is removed from `DependencyGraph::wait_results`
        // and as such we are keeping `from_stack` alive.
        let condvar = unsafe { me.add_edge(from_id, database_key, to_id, from_stack) };

        // Release the mutex that prevents `database_key`
        // from completing, now that the edge has been added.
        drop(query_mutex_guard);

        loop {
            if let Some(result) = me.wait_results.remove(&from_id) {
                debug_assert!(!me.edges.contains_key(&from_id));
                return result;
            }
            condvar.wait(&mut me);
        }
    }

    /// Helper for `block_on`: performs actual graph modification
    /// to add a dependency edge from `from_id` to `to_id`, which is
    /// computing `database_key`.
    ///
    /// # Safety
    ///
    /// The caller needs to keep `from_stack`/`'aq`` alive until `from_id` has been removed from the `wait_results`.
    // This safety invariant is consumed by the `Edge` struct
    unsafe fn add_edge<'aq>(
        &mut self,
        from_id: ThreadId,
        database_key: DatabaseKeyIndex,
        to_id: ThreadId,
        from_stack: &'aq mut [ActiveQuery],
    ) -> edge::EdgeGuard<'aq> {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));
        debug_assert!(!self.depends_on(to_id, from_id));
        // SAFETY: The caller is responsible for ensuring that the `EdgeGuard` outlives the `Edge`.
        let (edge, guard) = unsafe { edge::Edge::new(to_id, database_key, from_stack) };
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
    use std::{marker::PhantomData, mem, sync::Arc, thread::ThreadId};

    use parking_lot::MutexGuard;

    use crate::{
        runtime::{dependency_graph::DependencyGraph, ActiveQuery},
        DatabaseKeyIndex,
    };

    #[derive(Debug)]
    pub(super) struct Edge {
        pub(super) blocked_on_id: ThreadId,
        pub(super) blocked_on_key: DatabaseKeyIndex,
        // the 'static is a lie, we erased the actual lifetime here
        stack: &'static mut [ActiveQuery],

        /// Signalled whenever a query with dependents completes.
        /// Allows those dependents to check if they are ready to unblock.
        condvar: Arc<parking_lot::Condvar>,
    }

    pub struct EdgeGuard<'aq> {
        condvar: Arc<parking_lot::Condvar>,
        // Inform the borrow checker that the edge stack is borrowed until the guard is released.
        // This is necessary to ensure that the stack is not modified by the caller of
        // `DependencyGraph::add_edge` after the call returns.
        _pd: PhantomData<&'aq mut ()>,
    }

    impl EdgeGuard<'_> {
        pub fn wait(&self, mutex_guard: &mut MutexGuard<'_, DependencyGraph>) {
            self.condvar.wait(mutex_guard)
        }
    }

    impl Edge {
        pub(super) unsafe fn new<'aq>(
            blocked_on_id: ThreadId,
            blocked_on_key: DatabaseKeyIndex,
            stack: &'aq mut [ActiveQuery],
        ) -> (Self, EdgeGuard<'aq>) {
            let condvar = Arc::new(parking_lot::Condvar::new());
            let edge = Self {
                blocked_on_id,
                blocked_on_key,
                // SAFETY: We erase the lifetime here, the caller is responsible for ensuring that
                // the `EdgeGuard` outlives this `Edge`.
                stack: unsafe {
                    mem::transmute::<&'aq mut [ActiveQuery], &'static mut [ActiveQuery]>(stack)
                },
                condvar: condvar.clone(),
            };
            let edge_guard = EdgeGuard {
                condvar,
                _pd: PhantomData,
            };
            (edge, edge_guard)
        }

        pub(super) fn stack_mut(&mut self) -> &mut [ActiveQuery] {
            self.stack
        }

        pub(super) fn stack(&self) -> &[ActiveQuery] {
            self.stack
        }

        pub(super) fn notify(self) {
            self.condvar.notify_one();
        }
    }
}

fn strip_prefix_query_stack(stack_mut: &[ActiveQuery], key: DatabaseKeyIndex) -> &[ActiveQuery] {
    let prefix = stack_mut
        .iter()
        .take_while(|p| p.database_key_index != key)
        .count();
    &stack_mut[prefix..]
}

fn strip_prefix_query_stack_mut(
    stack_mut: &mut [ActiveQuery],
    key: DatabaseKeyIndex,
) -> &mut [ActiveQuery] {
    let prefix = stack_mut
        .iter()
        .take_while(|p| p.database_key_index != key)
        .count();
    &mut stack_mut[prefix..]
}
