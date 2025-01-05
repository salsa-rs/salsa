use std::any::TypeId;

use crate::memo_ingredient_indices::IngredientIndices;
use crate::zalsa::Zalsa;
use crate::Id;

pub trait SalsaStructInDb: Sized {
    /// This method is used to create ingredient indices. Note, it does *not* create the ingredients
    /// themselves, that is the job of [`Zalsa::add_or_lookup_jar_by_type()`]. This method only creates
    /// or lookup the indices. Naturally, implementors may call `add_or_lookup_jar_by_type()` to
    /// create the ingredient, but they do not must, e.g. enums recursively call
    /// `lookup_or_create_ingredient_index()` for their variants and combine them.
    fn lookup_or_create_ingredient_index(zalsa: &Zalsa) -> IngredientIndices;

    /// This method is used to support nested Salsa enums, a.k.a.:
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
    /// Imagine `OuterEnum` got a [`salsa::Id`][Id] and it wants to know which variant it belongs to.
    /// It cannot ask each variant "what is your ingredient index?" and compare, because `InnerEnum`
    /// has multiple possible ingredient indices.
    ///
    /// It could ask each variant "is this value yours?" and then invoke [`FromId`][crate::id::FromId]
    /// with the correct variant, but that will duplicate the work: now `InnerEnum` will have to do
    /// the same thing for its variants.
    ///
    /// Instead, we keep track of the [`TypeId`] of the ID struct, and ask each variant to "cast" it. If
    /// it succeeds, we return that value; if not, we go to the next variant.
    ///
    /// Why `TypeId` and not `IngredientIndex`? Because it's cheaper and easier. The `TypeId` is readily
    /// available at compile time, while the `IngredientIndex` requires a runtime lookup.
    fn cast(id: Id, type_id: TypeId) -> Option<Self>;
}
