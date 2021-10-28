use crate::{DatabaseKeyIndex, RuntimeId};
use parking_lot::MutexGuard;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

#[derive(Debug, Default)]
pub(super) struct DependencyGraph {
    /// A `(K -> V)` pair in this map indicates that the the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: FxHashMap<RuntimeId, Edge>,

    /// Encodes the `RuntimeId` that are blocked waiting for the result
    /// of a given query.
    query_dependents: FxHashMap<DatabaseKeyIndex, SmallVec<[RuntimeId; 4]>>,
}

#[derive(Debug)]
struct Edge {
    id: RuntimeId,
    path: Vec<DatabaseKeyIndex>,
}

impl DependencyGraph {
    /// True if `from_id` depends on `to_id`.
    ///
    /// (i.e., there is a path from `from_id` to `to_id` in the graph.)
    pub(super) fn depends_on(&mut self, from_id: RuntimeId, to_id: RuntimeId) -> bool {
        let mut p = from_id;
        while let Some(q) = self.edges.get(&p).map(|edge| edge.id) {
            if q == to_id {
                return true;
            }

            p = q;
        }
        false
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
    /// * Read lock (or stronger) on `database_key` table is held
    pub(super) fn block_on(
        mut me: MutexGuard<'_, Self>,
        from_id: RuntimeId,
        database_key: DatabaseKeyIndex,
        to_id: RuntimeId,
        path: impl IntoIterator<Item = DatabaseKeyIndex>,
    ) {
        me.add_edge(from_id, database_key, to_id, path);
    }

    /// Helper for `block_on`: performs actual graph modification
    /// to add a dependency edge from `from_id` to `to_id`, which is
    /// computing `database_key`.
    fn add_edge(
        &mut self,
        from_id: RuntimeId,
        database_key: DatabaseKeyIndex,
        to_id: RuntimeId,
        path: impl IntoIterator<Item = DatabaseKeyIndex>,
    ) {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));
        debug_assert!(!self.depends_on(to_id, from_id));

        self.edges.insert(
            from_id,
            Edge {
                id: to_id,
                path: path.into_iter().chain(Some(database_key.clone())).collect(),
            },
        );
        self.query_dependents
            .entry(database_key.clone())
            .or_default()
            .push(from_id);
    }

    pub(super) fn remove_edge(&mut self, database_key: DatabaseKeyIndex, to_id: RuntimeId) {
        let vec = self
            .query_dependents
            .remove(&database_key)
            .unwrap_or_default();

        for from_id in &vec {
            let to_id1 = self.edges.remove(from_id).map(|edge| edge.id);
            assert_eq!(Some(to_id), to_id1);
        }
    }

    pub(super) fn push_cycle_path(
        &self,
        database_key: DatabaseKeyIndex,
        to: RuntimeId,
        local_path: impl IntoIterator<Item = DatabaseKeyIndex>,
        output: &mut Vec<DatabaseKeyIndex>,
    ) {
        let mut current = Some((to, std::slice::from_ref(&database_key)));
        let mut last = None;
        let mut local_path = Some(local_path);

        loop {
            match current.take() {
                Some((id, path)) => {
                    let link_key = path.last().unwrap();

                    output.extend(path.iter().cloned());

                    current = self.edges.get(&id).map(|edge| {
                        let i = edge.path.iter().rposition(|p| p == link_key).unwrap();
                        (edge.id, &edge.path[i + 1..])
                    });

                    if current.is_none() {
                        last = local_path.take().map(|local_path| {
                            local_path
                                .into_iter()
                                .skip_while(move |p| *p != *link_key)
                                .skip(1)
                        });
                    }
                }
                None => break,
            }
        }

        if let Some(iter) = &mut last {
            output.extend(iter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dki(n: u32) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            group_index: 0,
            query_index: 0,
            key_index: n,
        }
    }

    macro_rules! dkivec {
        ($($n:expr),*) => {
            vec![$(dki($n)),*]
        }
    }

    #[test]
    fn dependency_graph_path1() {
        let mut graph = DependencyGraph::default();
        let a = RuntimeId { counter: 0 };
        let b = RuntimeId { counter: 1 };
        graph.add_edge(a, dki(2), b, dkivec![1]);
        let mut v = vec![];
        graph.push_cycle_path(dki(1), a, dkivec![3, 2], &mut v);
        assert_eq!(v, vec![dki(1), dki(2)]);
    }

    #[test]
    fn dependency_graph_path2() {
        let mut graph = DependencyGraph::default();
        let a = RuntimeId { counter: 0 };
        let b = RuntimeId { counter: 1 };
        let c = RuntimeId { counter: 2 };
        graph.add_edge(a, dki(3), b, dkivec![1]);
        graph.add_edge(b, dki(4), c, dkivec![2, 3]);
        // assert!(graph.add_edge(c, &1, a, vec![5, 6, 4, 7]));
        let mut v = vec![];
        graph.push_cycle_path(dki(1), a, dkivec![5, 6, 4, 7], &mut v);
        assert_eq!(v, dkivec![1, 3, 4, 7]);
    }
}
