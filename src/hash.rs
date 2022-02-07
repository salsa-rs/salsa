pub(crate) type FxHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;
pub(crate) type FxIndexSet<K> = indexmap::IndexSet<K, FxHasher>;
pub(crate) type FxIndexMap<K, V> = indexmap::IndexMap<K, V, FxHasher>;
pub(crate) type FxDashMap<K, V> = dashmap::DashMap<K, V, FxHasher>;
pub(crate) type FxLinkedHashSet<K> = hashlink::LinkedHashSet<K, FxHasher>;
