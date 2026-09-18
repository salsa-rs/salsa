use std::hash::{BuildHasher, Hash, Hasher};

// Clearing a retained hash table scans its entire control-byte array, making cleanup O(capacity)
// even when few entries remain. Below 16K, that scan is cheaper than allocator churn. Above it,
// retain up to 2x slack because geometric growth can naturally leave a new table about half full.
const MIN_INDEX_CAPACITY_TO_DISCARD: usize = 1 << 14;
const RETAINED_INDEX_CAPACITY_FACTOR: usize = 2;

pub(crate) type FxHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;
pub(crate) type FxIndexSet<K> = indexmap::IndexSet<K, FxHasher>;
pub(crate) type FxLinkedHashSet<K> = hashlink::LinkedHashSet<K, FxHasher>;
pub(crate) type FxHashSet<K> = std::collections::HashSet<K, FxHasher>;

pub(crate) fn should_discard_retained_capacity(len: usize, capacity: usize) -> bool {
    capacity >= MIN_INDEX_CAPACITY_TO_DISCARD
        && capacity > len.saturating_mul(RETAINED_INDEX_CAPACITY_FACTOR)
}

pub(crate) fn hash<T: Hash>(t: &T) -> u64 {
    FxHasher::default().hash_one(t)
}

// `TypeId` is a 128-bit hash internally, and it's `Hash` implementation
// writes the lower 64-bits. Hashing it again would be unnecessary.
#[derive(Default)]
pub(crate) struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("`TypeId` calls `write_u64`");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}
