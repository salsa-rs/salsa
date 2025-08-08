use std::any::TypeId;

use crate::memo_ingredient_indices::{IngredientIndices, MemoIngredientMap};
use crate::table::memo::MemoTableWithTypes;
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, Id, Revision};

pub trait SalsaStructInDb: Sized {
    type MemoIngredientMap: MemoIngredientMap;

    /// Lookup or create ingredient indices.
    ///
    /// Note that this method does *not* create the ingredients themselves, this is handled by
    /// [`crate::zalsa::JarEntry::get_or_create`]. This method only creates
    /// or looks up the indices corresponding to the ingredients.
    ///
    /// While implementors of this trait may call [`crate::zalsa::JarEntry::get_or_create`]
    /// to create the ingredient, they aren't required to. For example, supertypes recursively
    /// call [`crate::zalsa::JarEntry::get_or_create`] for their variants and combine them.
    fn lookup_ingredient_index(zalsa: &Zalsa) -> IngredientIndices;

    /// Returns the IDs of any instances of this struct in the database.
    fn entries(zalsa: &Zalsa) -> impl Iterator<Item = DatabaseKeyIndex> + '_;

    /// Plumbing to support nested salsa supertypes.
    ///
    /// In the example below, there are two supertypes: `InnerEnum` and `OuterEnum`,
    /// where the former is a supertype of `Input` and `Interned1` and the latter
    /// is a supertype of `InnerEnum` and `Interned2`.
    ///
    /// ```ignore
    /// #[salsa::input]
    /// struct Input {}
    ///
    /// #[salsa::interned]
    /// struct Interned1 {}
    ///
    /// #[salsa::interned]
    /// struct Interned2 {}
    ///
    /// #[derive(Debug, salsa::Enum)]
    /// enum InnerEnum {
    ///     Input(Input),
    ///     Interned1(Interned1),
    /// }
    ///
    /// #[derive(Debug, salsa::Enum)]
    /// enum OuterEnum {
    ///     InnerEnum(InnerEnum),
    ///     Interned2(Interned2),
    /// }
    /// ```
    ///
    /// Imagine `OuterEnum` got a [`salsa::Id`][Id] and it wants to know which variant it belongs to.
    ///
    /// `OuterEnum` cannot ask each variant "what is your ingredient index?" and compare because `InnerEnum`
    /// has *multiple*, possible ingredient indices. Alternatively, `OuterEnum` could ask eaach variant
    /// "is this value yours?" and then invoke [`FromId`][crate::id::FromId] with the correct variant,
    /// but this duplicates work: now, `InnerEnum` will have to repeat this check-and-cast for *its*
    /// variants.
    ///
    /// Instead, the implementor keeps track of the [`std::any::TypeId`] of the ID struct, and ask each
    /// variant to "cast" to it. If it succeeds, `cast` returns that value; if not, we
    /// go to the next variant.
    ///
    /// Why `TypeId` and not `IngredientIndex`? Because it's cheaper and easier: the `TypeId` is readily
    /// available at compile time, while the `IngredientIndex` requires a runtime lookup.
    fn cast(id: Id, type_id: TypeId) -> Option<Self>;

    /// Return the memo table associated with `id`.
    ///
    /// # Safety
    ///
    /// The parameter `current_revision` must be the current revision of the owner of database
    /// owning this table.
    unsafe fn memo_table(
        zalsa: &Zalsa,
        id: Id,
        current_revision: Revision,
    ) -> MemoTableWithTypes<'_>;
}
