use std::{
    borrow::Borrow,
    sync::{Arc, LazyLock},
};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    Database,
    plumbing::Ingredient,
    zalsa::{MemoIngredientIndex, Zalsa},
};

trait Serializable: Sized {
    fn serialize(&self, serializer: &mut Serializer);
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self;
}

impl Serializable for u8 {
    fn serialize(&self, serializer: &mut Serializer) {
        serializer.output.push(*self);
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        deserializer.input.split_off(..1).unwrap()[0]
    }
}

impl Serializable for u32 {
    fn serialize(&self, serializer: &mut Serializer) {
        serializer.output.extend(self.to_le_bytes());
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        let bytes;
        (bytes, deserializer.input) = deserializer.input.split_first_chunk().unwrap();
        Self::from_le_bytes(*bytes)
    }
}

impl Serializable for u64 {
    fn serialize(&self, serializer: &mut Serializer) {
        serializer.output.extend(self.to_le_bytes());
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        let bytes;
        (bytes, deserializer.input) = deserializer.input.split_first_chunk().unwrap();
        Self::from_le_bytes(*bytes)
    }
}

impl Serializable for usize {
    fn serialize(&self, serializer: &mut Serializer) {
        u64::try_from(*self).unwrap().serialize(serializer);
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        u64::deserialize(deserializer).try_into().unwrap()
    }
}

impl Serializable for MemoIngredientIndex {
    fn serialize(&self, serializer: &mut Serializer) {
        self.as_usize().serialize(serializer);
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        Self::from_usize(usize::deserialize(deserializer))
    }
}

impl<T: Serializable> Serializable for Vec<T> {
    fn serialize(&self, serializer: &mut Serializer) {
        self.len().serialize(serializer);
        for item in self {
            item.serialize(serializer);
        }
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        let len = usize::deserialize(deserializer);
        (0..len).map(|_| T::deserialize(deserializer)).collect()
    }
}

impl<K: std::hash::Hash + Eq + Serializable, V: Serializable> Serializable for FxHashMap<K, V> {
    fn serialize(&self, serializer: &mut Serializer) {
        self.len().serialize(serializer);
        for (key, value) in self {
            key.serialize(serializer);
            value.serialize(serializer);
        }
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        let len = usize::deserialize(deserializer);
        (0..len)
            .map(|_| (K::deserialize(deserializer), V::deserialize(deserializer)))
            .collect()
    }
}

#[derive(Default)]
struct Serializer {
    output: Vec<u8>,
}

struct Deserializer<'a> {
    input: &'a [u8],
}

/// We cannot rely on the `IngredientIndex` to stay stable, so we use the ingredient's debug name instead,
/// which is based on [`std::any::type_name()`]. While that function doesn't have any guarantee, in practice
/// this works, and this is only an optimization so mismatches won't cause any harm.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct StableIngredientName(Vec<u8>);

impl StableIngredientName {
    fn from_ingredient(ingredient: &dyn Ingredient) -> Self {
        Self(ingredient.debug_name().as_bytes().to_vec())
    }
}

impl Serializable for StableIngredientName {
    fn serialize(&self, serializer: &mut Serializer) {
        self.0.serialize(serializer);
    }

    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        Self(Serializable::deserialize(deserializer))
    }
}

impl std::fmt::Debug for StableIngredientName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::str::from_utf8(&self.0).unwrap().fmt(f)
    }
}

impl Borrow<[u8]> for StableIngredientName {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug)]
pub(crate) struct StructIngredientData {
    /// The length that a memo table will have, if all memos defined in this stats are initialized.
    /// This is used to push additional memos that do not appear in the stats (can occur for mismatching code,
    /// or when the stats sample wasn't representative).
    pub(crate) max_memo_len: usize,
    /// The minimum length for a memo table (that is not empty). Since some memos are commonly allocated together
    /// for most instances, we want to give some minimal length to avoid reallocations.
    ///
    /// The user determines the threshold (see [`MemoFrequencyStats::determine_from_db()`]).
    pub(crate) optimal_init_len: usize,
    pub(crate) memos: FxHashMap<StableIngredientName, MemoIngredientIndex>,
}

impl Serializable for StructIngredientData {
    fn serialize(&self, serializer: &mut Serializer) {
        // Ignore `max_memo_len`, we derive it from the serialized data because it can be UB (duplicate memo ingredient indices)
        // if it's wrong.
        let Self {
            max_memo_len: _,
            optimal_init_len,
            memos,
        } = self;
        optimal_init_len.serialize(serializer);
        memos.serialize(serializer);
    }
    fn deserialize(deserializer: &mut Deserializer<'_>) -> Self {
        let optimal_init_len = Serializable::deserialize(deserializer);
        let memos = FxHashMap::<_, MemoIngredientIndex>::deserialize(deserializer);

        let mut max_memo_len = 0;
        let mut unique_memo_indices = FxHashSet::default();
        for &memo_data in memos.values() {
            max_memo_len = max_memo_len.max(memo_data.as_usize() + 1);
            assert!(
                unique_memo_indices.insert(memo_data),
                "duplicate MemoIngredientIndex"
            );
        }

        Self {
            max_memo_len,
            optimal_init_len,
            memos,
        }
    }
}

/// A struct containing statistics about the frequency of memos in a database.
///
/// This is a bit like PGO: you are supposed to run some workload, then get this struct via
/// [`MemoFrequencyStats::determine_from_db()`]. You should then [serialize] it and save
/// to a file, and then in later runs you [deserialize] this struct from the file and
/// give it to the storage builder, which will use it to optimize memory usage - more common
/// memos will come first, so we can avoid allocating space for less common memos.
///
/// [serialize]: MemoFrequencyStats::serialize
/// [deserialize]: MemoFrequencyStats::deserialize
#[derive(Debug, Clone)]
pub struct MemoFrequencyStats {
    pub(crate) best_order: Arc<FxHashMap<StableIngredientName, StructIngredientData>>,
}

impl Default for MemoFrequencyStats {
    #[inline]
    fn default() -> Self {
        static EMPTY: LazyLock<MemoFrequencyStats> = LazyLock::new(|| MemoFrequencyStats {
            best_order: Default::default(),
        });
        EMPTY.clone()
    }
}

impl MemoFrequencyStats {
    /// Serializes this struct into `Vec<u8>`, which can be written to a file.
    pub fn serialize(&self) -> Vec<u8> {
        let mut serializer = Serializer::default();
        self.best_order.serialize(&mut serializer);
        serializer.output
    }

    /// Deserializes this struct.
    ///
    /// # Panics
    ///
    /// This will panic if the format does not match.
    pub fn deserialize(input: &[u8]) -> Self {
        let mut deserializer = Deserializer { input };
        Self {
            best_order: Arc::new(Serializable::deserialize(&mut deserializer)),
        }
    }

    /// Retrieves the statistics of `db`.
    ///
    /// `optimal_initial_fraction` decides the threshold for a memo to be preallocated. It should be between 0 or 1
    /// (nor error is given for values outside the range but will be meaningless, either preallocating all or none).
    /// If the ratio of some memo in the total number of allocated memo tables is at least this number, then we
    /// save space for it in any non-empty memo table.
    pub fn determine_from_db(db: &(impl Database + ?Sized), optimal_initial_fraction: f64) -> Self {
        Self::determine_from_zalsa(db.zalsa(), optimal_initial_fraction)
    }

    fn determine_from_zalsa(zalsa: &Zalsa, optimal_initial_fraction: f64) -> Self {
        let mut memo_counts = FxHashMap::default();
        for ingredient in zalsa.ingredients() {
            let counts = ingredient.memo_counts(zalsa);
            memo_counts.insert(StableIngredientName::from_ingredient(ingredient), counts);
        }
        let best_order = memo_counts
            .into_iter()
            .filter(|(_, (_, counts))| !counts.is_empty())
            .map(|(struct_ingredient, (total_count, mut counts))| {
                counts.sort_unstable_by_key(|&(_, count)| std::cmp::Reverse(count));

                let memos = counts
                    .iter()
                    .enumerate()
                    .map(|(idx, &(memo, _))| {
                        let memo_ingredient =
                            StableIngredientName::from_ingredient(zalsa.lookup_ingredient(memo));
                        let memo_ingredient_index = MemoIngredientIndex::from_usize(idx);
                        (memo_ingredient, memo_ingredient_index)
                    })
                    .collect();

                let max_memo_len = counts.len();

                let total_count = total_count as f64;
                let optimal_init_len = counts
                    .iter()
                    .position(|&(_, count)| count as f64 / total_count < optimal_initial_fraction)
                    .unwrap_or(counts.len());

                (
                    struct_ingredient,
                    StructIngredientData {
                        max_memo_len,
                        optimal_init_len,
                        memos,
                    },
                )
            })
            .collect();
        Self {
            best_order: Arc::new(best_order),
        }
    }
}
