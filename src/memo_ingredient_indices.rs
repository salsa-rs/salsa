use crate::table::Table;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::{Id, IngredientIndex};

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

impl From<(&Zalsa, IngredientIndices, IngredientIndex)> for MemoIngredientIndices {
    #[inline]
    fn from(
        (zalsa, struct_indices, ingredient): (&Zalsa, IngredientIndices, IngredientIndex),
    ) -> Self {
        let Some(&last) = struct_indices.indices.last() else {
            unreachable!("Attempting to construct struct memo mapping for non tracked function?")
        };
        let mut indices = Vec::new();
        indices.resize(
            last.as_usize() + 1,
            MemoIngredientIndex::from_usize((u32::MAX - 1) as usize),
        );
        for &struct_ingredient in &struct_indices.indices {
            indices[struct_ingredient.as_usize()] =
                zalsa.next_memo_ingredient_index(struct_ingredient, ingredient);
        }
        MemoIngredientIndices {
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
/// A database tends to have few ingredients (i), less function ingredients and even less
/// function ingredients targeting `#[derive(Supertype)]` enums (e).
/// While this is bounded as `O(i * e)` memory usage, the average case is significantly smaller: a
/// function ingredient targeting enums only stores a slice whose length corresponds to the largest
/// ingredient index's _value_. For example, if we have the ingredient indices `[2, 6, 17]`, then we
/// will allocate a slice whose length is `17 + 1`.
///
/// Assuming a heavy example scenario of 1000 ingredients (500 of which are function ingredients, 100
/// of which are enum targeting functions) this would come out to a maximum possibly memory usage of
/// 4bytes * 1000 * 100 ~= 0.38MB which is negligible.
pub struct MemoIngredientIndices {
    indices: Box<[MemoIngredientIndex]>,
}

impl MemoIngredientMap for MemoIngredientIndices {
    #[inline(always)]
    fn get_id_with_table(&self, table: &Table, id: Id) -> MemoIngredientIndex {
        self.get(table.ingredient_index(id))
    }

    #[inline(always)]
    fn get(&self, index: IngredientIndex) -> MemoIngredientIndex {
        self.indices[index.as_usize()]
    }
}

#[derive(Debug)]
pub struct MemoIngredientSingletonIndex(MemoIngredientIndex);

impl MemoIngredientMap for MemoIngredientSingletonIndex {
    #[inline(always)]
    fn get_id_with_table(&self, _: &Table, _: Id) -> MemoIngredientIndex {
        self.0
    }

    #[inline(always)]
    fn get(&self, _: IngredientIndex) -> MemoIngredientIndex {
        self.0
    }
}

impl From<(&Zalsa, IngredientIndices, IngredientIndex)> for MemoIngredientSingletonIndex {
    #[inline]
    fn from((zalsa, indices, ingredient): (&Zalsa, IngredientIndices, IngredientIndex)) -> Self {
        let &[struct_ingredient] = &*indices.indices else {
            unreachable!("Attempting to construct struct memo mapping from enum?")
        };

        Self(zalsa.next_memo_ingredient_index(struct_ingredient, ingredient))
    }
}

pub trait MemoIngredientMap: Send + Sync {
    fn get_id_with_table(&self, table: &Table, id: Id) -> MemoIngredientIndex;
    fn get(&self, index: IngredientIndex) -> MemoIngredientIndex;
}
