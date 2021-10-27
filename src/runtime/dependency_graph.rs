use crate::RuntimeId;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::hash::Hash;

#[derive(Debug)]
pub(super) struct DependencyGraph<K: Hash + Eq> {
    /// A `(K -> V)` pair in this map indicates that the the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: FxHashMap<RuntimeId, Edge<K>>,
    labels: FxHashMap<K, SmallVec<[RuntimeId; 4]>>,
}

#[derive(Debug)]
struct Edge<K> {
    id: RuntimeId,
    path: Vec<K>,
}

impl<K> Default for DependencyGraph<K>
where
    K: Hash + Eq,
{
    fn default() -> Self {
        DependencyGraph {
            edges: Default::default(),
            labels: Default::default(),
        }
    }
}

impl<K> DependencyGraph<K>
where
    K: Hash + Eq + Clone,
{
    /// Attempt to add an edge `from_id -> to_id` into the result graph.
    pub(super) fn add_edge(
        &mut self,
        from_id: RuntimeId,
        database_key: K,
        to_id: RuntimeId,
        path: impl IntoIterator<Item = K>,
    ) -> bool {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));

        // First: walk the chain of things that `to_id` depends on,
        // looking for us.
        let mut p = to_id;
        while let Some(q) = self.edges.get(&p).map(|edge| edge.id) {
            if q == from_id {
                return false;
            }

            p = q;
        }

        self.edges.insert(
            from_id,
            Edge {
                id: to_id,
                path: path.into_iter().chain(Some(database_key.clone())).collect(),
            },
        );
        self.labels
            .entry(database_key.clone())
            .or_default()
            .push(from_id);
        true
    }

    pub(super) fn remove_edge(&mut self, database_key: K, to_id: RuntimeId) {
        let vec = self.labels.remove(&database_key).unwrap_or_default();

        for from_id in &vec {
            let to_id1 = self.edges.remove(from_id).map(|edge| edge.id);
            assert_eq!(Some(to_id), to_id1);
        }
    }

    pub(super) fn push_cycle_path(
        &self,
        database_key: K,
        to: RuntimeId,
        local_path: impl IntoIterator<Item = K>,
        output: &mut Vec<K>,
    ) where
        K: std::fmt::Debug,
    {
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
