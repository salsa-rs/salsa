use std::{
    any::{Any, TypeId},
    fmt,
};

use crate::{
    accumulator::accumulated_map::AccumulatedMap,
    cycle::CycleRecoveryStrategy,
    zalsa::{IngredientIndex, MemoIngredientIndex},
    zalsa_local::QueryOrigin,
    Database, DatabaseKeyIndex, Id,
};

use super::Revision;

/// A "jar" is a group of ingredients that are added atomically.
/// Each type implementing jar can be added to the database at most once.
pub trait Jar: Any {
    /// Create the ingredients given the index of the first one.
    /// All subsequent ingredients will be assigned contiguous indices.
    fn create_ingredients(
        &self,
        aux: &dyn JarAux,
        first_index: IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>>;
}

pub trait JarAux {
    fn next_memo_ingredient_index(&self, ingredient_index: IngredientIndex) -> MemoIngredientIndex;
}

pub trait Ingredient: Any + std::fmt::Debug + Send + Sync {
    fn debug_name(&self) -> &'static str;

    /// Has the value for `input` in this ingredient changed after `revision`?
    fn maybe_changed_after<'db>(
        &'db self,
        db: &'db dyn Database,
        input: Id,
        revision: Revision,
    ) -> bool;

    /// What were the inputs (if any) that were used to create the value at `key_index`.
    fn origin(&self, db: &dyn Database, key_index: Id) -> Option<QueryOrigin>;

    /// What values were accumulated during the creation of the value at `key_index`
    /// (if any).
    ///
    /// In practice, returns `Some` only for tracked function ingredients.
    fn accumulated<'db>(
        &'db self,
        db: &'db dyn Database,
        key_index: Id,
    ) -> Option<&'db AccumulatedMap>;

    /// Invoked when the value `output_key` should be marked as valid in the current revision.
    /// This occurs because the value for `executor`, which generated it, was marked as valid
    /// in the current revision.
    fn mark_validated_output<'db>(
        &'db self,
        db: &'db dyn Database,
        executor: DatabaseKeyIndex,
        output_key: Id,
    );

    /// Invoked when the value `stale_output` was output by `executor` in a previous
    /// revision, but was NOT output in the current revision.
    ///
    /// This hook is used to clear out the stale value so others cannot read it.
    fn remove_stale_output(
        &self,
        db: &dyn Database,
        executor: DatabaseKeyIndex,
        stale_output_key: Id,
    );

    /// Returns the [`IngredientIndex`] of this ingredient.
    fn ingredient_index(&self) -> IngredientIndex;

    /// If this ingredient is a participant in a cycle, what is its cycle recovery strategy?
    /// (Really only relevant to [`crate::function::FunctionIngredient`],
    /// since only function ingredients push themselves onto the active query stack.)
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy;

    /// Returns true if `reset_for_new_revision` should be called when new revisions start.
    /// Invoked once when ingredient is added and not after that.
    fn requires_reset_for_new_revision(&self) -> bool;

    /// Invoked when a new revision is about to start.
    /// This moment is important because it means that we have an `&mut`-reference to the
    /// database, and hence any pre-existing `&`-references must have expired.
    /// Many ingredients, given an `&'db`-reference to the database,
    /// use unsafe code to return `&'db`-references to internal values.
    /// The backing memory for those values can only be freed once an `&mut`-reference to the
    /// database is created.
    ///
    /// **Important:** to actually receive resets, the ingredient must set
    /// [`IngredientRequiresReset::RESET_ON_NEW_REVISION`] to true.
    fn reset_for_new_revision(&mut self);

    fn fmt_index(&self, index: Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl dyn Ingredient {
    /// Equivalent to the `downcast` methods on `any`.
    /// Because we do not have dyn-upcasting support, we need this workaround.
    pub fn assert_type<T: Any>(&self) -> &T {
        assert_eq!(
            self.type_id(),
            TypeId::of::<T>(),
            "ingredient `{self:?}` is not of type `{}`",
            std::any::type_name::<T>()
        );

        // SAFETY: We know that the underlying data pointer
        // refers to a value of type T because of the `TypeId` check above.
        let this: *const dyn Ingredient = self;
        let this = this as *const T; // discards the vtable
        unsafe { &*this }
    }

    /// Equivalent to the `downcast` methods on `any`.
    /// Because we do not have dyn-upcasting support, we need this workaround.
    pub fn assert_type_mut<T: Any>(&mut self) -> &mut T {
        assert_eq!(
            Any::type_id(self),
            TypeId::of::<T>(),
            "ingredient `{self:?}` is not of type `{}`",
            std::any::type_name::<T>()
        );

        // SAFETY: We know that the underlying data pointer
        // refers to a value of type T because of the `TypeId` check above.
        let this: *mut dyn Ingredient = self;
        let this = this as *mut T; // discards the vtable
        unsafe { &mut *this }
    }
}

/// A helper function to show human readable fmt.
pub(crate) fn fmt_index(debug_name: &str, id: Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(fmt, "{debug_name}({id:?})")
}
