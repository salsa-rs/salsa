use crossbeam_utils::atomic::AtomicCell;
use std::borrow::Borrow;
use std::hash::Hash;

use crate::hash::FxDashMap;

use super::DerivedKeyIndex;

pub(super) struct KeyToKeyIndex<K> {
    index_map: FxDashMap<K, DerivedKeyIndex>,
    key_map: FxDashMap<DerivedKeyIndex, K>,
    indices: AtomicCell<u32>,
}

impl<K> Default for KeyToKeyIndex<K>
where
    K: Hash + Eq,
{
    fn default() -> Self {
        Self {
            index_map: Default::default(),
            key_map: Default::default(),
            indices: Default::default(),
        }
    }
}

impl<K> KeyToKeyIndex<K>
where
    K: Hash + Eq + Clone,
{
    pub(super) fn key_index_for_key(&self, key: &K) -> DerivedKeyIndex {
        // Common case: get an existing key
        if let Some(v) = self.index_map.get(key) {
            return *v;
        }

        // Less common case: (potentially) create a new slot
        *self.index_map.entry(key.clone()).or_insert_with(|| {
            let key_index = self.indices.fetch_add(1);
            self.key_map.insert(key_index, key.clone());
            key_index
        })
    }

    pub(super) fn existing_key_index_for_key<S>(&self, key: &S) -> Option<DerivedKeyIndex>
    where
        S: Eq + Hash,
        K: Borrow<S>,
    {
        // Common case: get an existing key
        if let Some(v) = self.index_map.get(key) {
            Some(*v)
        } else {
            None
        }
    }

    pub(super) fn key_for_key_index(&self, key_index: DerivedKeyIndex) -> K {
        self.key_map.get(&key_index).unwrap().clone()
    }
}
