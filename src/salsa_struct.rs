use std::any::TypeId;

use crate::memo_ingredient_indices::IngredientIndices;
use crate::zalsa::Zalsa;
use crate::Id;

pub trait SalsaStructInDb: Sized {
    type MemoIngredientMap: std::ops::Index<crate::IngredientIndex, Output = crate::zalsa::MemoIngredientIndex>
        + Send
        + Sync;

    /// Lookup or create ingredient indices.
    ///
    /// Note that this method does *not* create the ingredients themselves, this is handled by
    /// [`Zalsa::add_or_lookup_jar_by_type()`]. This method only creates
    /// or looks up the indices corresponding to the ingredients.
    ///
    /// While implementors of this trait may call [`Zalsa::add_or_lookup_jar_by_type()`]
    /// to create the ingredient, they aren't required to. For example, supertypes recursively
    /// call [`Zalsa::add_or_lookup_jar_by_type()`] for their variants and combine them.
    fn lookup_or_create_ingredient_index(zalsa: &Zalsa) -> IngredientIndices;

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
}
