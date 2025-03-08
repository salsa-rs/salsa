use std::{
    any::{Any, TypeId},
    fmt,
};

use crate::{
    accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues},
    cycle::CycleRecoveryStrategy,
    function::VerifyResult,
    plumbing::IngredientIndices,
    table::Table,
    zalsa::{transmute_data_mut_ptr, transmute_data_ptr, IngredientIndex, Zalsa},
    zalsa_local::QueryOrigin,
    Database, DatabaseKeyIndex, Id,
};

use super::Revision;

/// A "jar" is a group of ingredients that are added atomically.
/// Each type implementing jar can be added to the database at most once.
pub trait Jar: Any {
    /// This creates the ingredient dependencies of this jar. We need to split this from `create_ingredients()`
    /// because while `create_ingredients()` is called, a lock on the ingredient map is held (to guarantee
    /// atomicity), so other ingredients could not be created.
    ///
    /// Only tracked fns use this.
    fn create_dependencies(_zalsa: &Zalsa) -> IngredientIndices
    where
        Self: Sized,
    {
        IngredientIndices::empty()
    }

    /// Create the ingredients given the index of the first one.
    /// All subsequent ingredients will be assigned contiguous indices.
    fn create_ingredients(
        zalsa: &Zalsa,
        first_index: IngredientIndex,
        dependencies: IngredientIndices,
    ) -> Vec<Box<dyn Ingredient>>
    where
        Self: Sized;

    /// This returns the [`TypeId`] of the ID struct, that is, the struct that wraps `salsa::Id`
    /// and carry the name of the jar.
    fn id_struct_type_id() -> TypeId
    where
        Self: Sized;
}

pub trait Ingredient: Any + std::fmt::Debug + Send + Sync {
    fn debug_name(&self) -> &'static str;

    /// Has the value for `input` in this ingredient changed after `revision`?
    ///
    /// # Safety
    ///
    /// The passed in database needs to be the same one that the ingredient was created with.
    unsafe fn maybe_changed_after<'db>(
        &'db self,
        db: &'db dyn Database,
        input: Id,
        revision: Revision,
    ) -> VerifyResult;

    /// Is the value for `input` in this ingredient a cycle head that is still provisional?
    ///
    /// In the case of nested cycles, we are not asking here whether the value is provisional due
    /// to the outer cycle being unresolved, only whether its own cycle remains provisional.
    fn is_provisional_cycle_head<'db>(&'db self, db: &'db dyn Database, input: Id) -> bool;

    /// Invoked when the current thread needs to wait for a result for the given `key_index`.
    ///
    /// A return value of `true` indicates that a result is now available. A return value of
    /// `false` means that a cycle was encountered; the waited-on query is either already claimed
    /// by the current thread, or by a thread waiting on the current thread.
    fn wait_for(&self, db: &dyn Database, key_index: Id) -> bool;

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
    ) -> (Option<&'db AccumulatedMap>, InputAccumulatedValues) {
        _ = (db, key_index);
        (None, InputAccumulatedValues::Any)
    }

    /// Invoked when the value `output_key` should be marked as valid in the current revision.
    /// This occurs because the value for `executor`, which generated it, was marked as valid
    /// in the current revision.
    fn mark_validated_output<'db>(
        &'db self,
        db: &'db dyn Database,
        executor: DatabaseKeyIndex,
        output_key: crate::Id,
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
        provisional: bool,
    );

    /// Returns the [`IngredientIndex`] of this ingredient.
    fn ingredient_index(&self) -> IngredientIndex;

    /// If this ingredient is a participant in a cycle, what is its cycle recovery strategy?
    /// (Really only relevant to [`crate::function::FunctionIngredient`],
    /// since only function ingredients push themselves onto the active query stack.)
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy;

    /// Returns true if `reset_for_new_revision` should be called when new revisions start.
    /// Invoked once when ingredient is added and not after that.
    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

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
    fn reset_for_new_revision(&mut self, table: &mut Table) {
        _ = table;
        panic!(
            "Ingredient `{}` set `Ingredient::requires_reset_for_new_revision` to true but does \
            not overwrite `Ingredient::reset_for_new_revision`",
            self.debug_name()
        );
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result;
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
        unsafe { transmute_data_ptr(self) }
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
        unsafe { transmute_data_mut_ptr(self) }
    }
}

/// A helper function to show human readable fmt.
pub(crate) fn fmt_index(
    debug_name: &str,
    id: Option<Id>,
    fmt: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    if let Some(i) = id {
        write!(fmt, "{debug_name}({i:?})")
    } else {
        write!(fmt, "{debug_name}()")
    }
}
