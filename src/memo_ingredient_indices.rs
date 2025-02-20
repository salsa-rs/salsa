use std::ops;

use crate::zalsa::MemoIngredientIndex;
use crate::IngredientIndex;

/// An ingredient has an [ingredient index][IngredientIndex]. However, Salsa also supports
/// enums of salsa structs (and other salsa enums), and those don't have a constant ingredient index,
/// because they are not ingredients by themselves but rather composed of them. However, an enum can
/// be viewed as a *set* of [`IngredientIndex`], where each instance of the enum can belong
/// to one, potentially different, index. This is what this type represents: a set of
/// `IngredientIndex`.
#[derive(Clone)]
pub struct IngredientIndices {
    indices: Box<[IngredientIndex]>,
}

impl From<IngredientIndex> for IngredientIndices {
    #[inline]
    fn from(value: IngredientIndex) -> Self {
        Self {
            indices: Box::new([value]),
        }
    }
}

impl IngredientIndices {
    #[inline]
    pub(crate) fn memo_indices(
        &self,
        mut memo_index: impl FnMut(IngredientIndex) -> MemoIngredientIndex,
    ) -> MemoIngredientIndices {
        let mut indices = Vec::new();
        let Some(&last) = self.indices.last() else {
            return MemoIngredientIndices {
                indices: Box::default(),
            };
        };
        indices.resize(
            last.as_usize() + 1,
            MemoIngredientIndex::from_usize((u32::MAX - 1) as usize),
        );
        for &ingredient in &self.indices {
            indices[ingredient.as_usize()] = memo_index(ingredient);
        }
        MemoIngredientIndices {
            indices: indices.into_boxed_slice(),
        }
    }

    #[inline]
    pub fn empty() -> Self {
        Self {
            indices: Box::default(),
        }
    }

    pub fn merge(iter: impl IntoIterator<Item = Self>) -> Self {
        let mut indices = Vec::new();
        for index in iter {
            indices.extend(index.indices);
        }
        indices.sort_unstable();
        indices.dedup();
        Self {
            indices: indices.into_boxed_slice(),
        }
    }
}

/// This type is to [`MemoIngredientIndex`] what [`IngredientIndices`] is to [`IngredientIndex`]:
/// since enums can contain different ingredient indices, they can also have different memo indices,
/// so we need to keep track of them.
///
/// This acts a map from [`IngredientIndex`] to [`MemoIngredientIndex`] but implemented
/// via a slice for fast lookups, trading memory for speed. With these changes, lookups are `O(1)`
/// instead of `O(n)`.
///
/// A database tends to have few ingredients (i) and even less function ingredients (f).
/// While this is bounded as `O(i * f)`, the average case is significantly smaller: a function
/// ingredient only stores a slice whose length corresponds to the largest ingredient index's
/// _value_. For example, if we have the ingredient indices `[2, 6, 17]`, then we will allocate a
/// slice whose length is `17 + 1`.
///
/// Assuming a heavy example scenario of 1000 ingredients (500 of which are function ingredients)
/// this would come out to a maximum possibly memory usage of 4bytes * 1000 * 500 ~= 1.9MB.
/// Given such a number scenario is already rather unlikely and the average usages of memory being
/// lower we can sacrifice some memory for speed.
#[derive(Clone)]
pub struct MemoIngredientIndices {
    indices: Box<[MemoIngredientIndex]>,
}

impl ops::Index<IngredientIndex> for MemoIngredientIndices {
    type Output = MemoIngredientIndex;

    fn index(&self, index: IngredientIndex) -> &Self::Output {
        &self.indices[index.as_usize()]
    }
}
