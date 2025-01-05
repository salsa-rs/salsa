use std::fmt;

use crate::zalsa::MemoIngredientIndex;
use crate::IngredientIndex;

/// The maximum number of memo ingredient indices we can hold. This affects the
/// maximum number of variants possible in `#[derive(salsa::Enum)]`. We use a const
/// so that we don't allocate and to perhaps allow the compiler to vectorize the search.
pub const MAX_MEMO_INGREDIENT_INDICES: usize = 20;

/// An ingredient has an [ingredient index][IngredientIndex]. However, Salsa also supports
/// enums of salsa structs, and those don't have a constant ingredient index, because they
/// are not ingredients by themselves but rather composed of them. However, an enum can be
/// viewed as a *set* of [`IngredientIndex`], where each instance of the enum can belong
/// to one, potentially different, index. This is what this type represents: a set of
/// `IngredientIndex`.
///
/// This type is represented as an array, for efficiency, and supports up to 20 indices.
/// That means that Salsa enums can have at most 20 variants. Alternatively, they can also
/// contain Salsa enums as variants, but then the total number of variants is counter - because
/// what matters is the number of unique `IngredientIndex`s.
#[derive(Clone)]
pub struct IngredientIndices {
    indices: [IngredientIndex; MAX_MEMO_INGREDIENT_INDICES],
    len: u8,
}

impl From<IngredientIndex> for IngredientIndices {
    #[inline]
    fn from(value: IngredientIndex) -> Self {
        let mut result = Self::uninitialized();
        result.indices[0] = value;
        result.len = 1;
        result
    }
}

impl fmt::Debug for IngredientIndices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(&self.indices[..self.len.into()])
            .finish()
    }
}

impl IngredientIndices {
    #[inline]
    pub(crate) fn memo_indices(
        &self,
        mut memo_index: impl FnMut(IngredientIndex) -> MemoIngredientIndex,
    ) -> MemoIngredientIndices {
        let mut memo_ingredient_indices = [(
            IngredientIndex::from((u32::MAX - 1) as usize),
            MemoIngredientIndex::from_usize((u32::MAX - 1) as usize),
        ); MAX_MEMO_INGREDIENT_INDICES];
        for i in 0..usize::from(self.len) {
            let memo_ingredient_index = memo_index(self.indices[i]);
            memo_ingredient_indices[i] = (self.indices[i], memo_ingredient_index);
        }
        MemoIngredientIndices {
            indices: memo_ingredient_indices,
            len: self.len,
        }
    }

    #[inline]
    pub fn uninitialized() -> Self {
        Self {
            indices: [IngredientIndex::from((u32::MAX - 1) as usize); MAX_MEMO_INGREDIENT_INDICES],
            len: 0,
        }
    }

    #[track_caller]
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        if usize::from(self.len) + usize::from(other.len) > MAX_MEMO_INGREDIENT_INDICES {
            panic!("too many variants in the salsa enum");
        }
        self.indices[usize::from(self.len)..][..usize::from(other.len)]
            .copy_from_slice(&other.indices[..usize::from(other.len)]);
        self.len += other.len;
    }
}

/// This type is to [`MemoIngredientIndex`] what [`IngredientIndices`] is to [`IngredientIndex`]:
/// since enums can contain different ingredient indices, they can also have different memo indices,
/// so we need to keep track of them.
#[derive(Clone)]
pub struct MemoIngredientIndices {
    indices: [(IngredientIndex, MemoIngredientIndex); MAX_MEMO_INGREDIENT_INDICES],
    len: u8,
}

impl fmt::Debug for MemoIngredientIndices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(&self.indices[..self.len.into()])
            .finish()
    }
}

impl MemoIngredientIndices {
    #[inline]
    pub(crate) fn find(&self, ingredient_index: IngredientIndex) -> MemoIngredientIndex {
        for &(ingredient, memo_ingredient_index) in &self.indices[..(self.len - 1).into()] {
            if ingredient == ingredient_index {
                return memo_ingredient_index;
            }
        }
        // It must be the last.
        self.indices[usize::from(self.len - 1)].1
    }
}
