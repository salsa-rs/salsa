use std::{any::Any, fmt, marker::PhantomData};

use crate::{
    cycle::CycleRecoveryStrategy, key::DependencyIndex, runtime::local_state::QueryOrigin,
    storage::IngredientIndex, Database, DatabaseKeyIndex, DatabaseView, Id, Revision,
};

use super::{Ingredient, RawIngredient};

/// Encapsulates an ingredient whose methods expect some database view that is supported by our database.
/// This is current implemented via double indirection.
/// We can theoretically implement more efficient methods in the future if that ever becomes worthwhile.
pub(crate) struct AdaptedIngredient<Db: Database> {
    ingredient: Box<dyn AdaptedIngredientTrait<DbView = Db>>,
}

impl<Db: Database> AdaptedIngredient<Db> {
    pub fn new<DbView>(ingredient: Box<dyn Ingredient<DbView = DbView>>) -> Self
    where
        Db: DatabaseView<DbView>,
        DbView: ?Sized + Any,
    {
        Self {
            ingredient: Box::new(AdaptedIngredientImpl::new(ingredient)),
        }
    }

    /// Return the raw version of the underlying, unadapted ingredient.
    pub fn unadapted_ingredient(&self) -> &dyn RawIngredient {
        self.ingredient.unadapted_ingredient()
    }

    /// Return the raw version of the underlying, unadapted ingredient.
    pub fn unadapted_ingredient_mut(&mut self) -> &mut dyn RawIngredient {
        self.ingredient.unadapted_ingredient_mut()
    }
}

/// This impl is kind of annoying, it just delegates to self.ingredient,
/// it is meant to be used with static dispatch and hence adds no overhead.
/// The only reason it exists is that it gives us the freedom to implement
/// `AdaptedIngredient` via some more efficient (but unsafe) means
/// in the future.
impl<Db: Database> Ingredient for AdaptedIngredient<Db> {
    type DbView = Db;

    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient.ingredient_index()
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        self.ingredient.cycle_recovery_strategy()
    }

    fn maybe_changed_after<'db>(
        &'db self,
        db: &'db Db,
        input: DependencyIndex,
        revision: Revision,
    ) -> bool {
        self.ingredient.maybe_changed_after(db, input, revision)
    }

    fn origin(&self, key_index: Id) -> Option<QueryOrigin> {
        self.ingredient.origin(key_index)
    }

    fn mark_validated_output<'db>(
        &'db self,
        db: &'db Db,
        executor: DatabaseKeyIndex,
        output_key: Option<Id>,
    ) {
        self.ingredient
            .mark_validated_output(db, executor, output_key)
    }

    fn remove_stale_output(
        &self,
        db: &Db,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<Id>,
    ) {
        self.ingredient
            .remove_stale_output(db, executor, stale_output_key)
    }

    fn salsa_struct_deleted(&self, db: &Self::DbView, id: Id) {
        self.ingredient.salsa_struct_deleted(db, id)
    }

    fn reset_for_new_revision(&mut self) {
        self.ingredient.reset_for_new_revision()
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.ingredient.fmt_index(index, fmt)
    }

    fn upcast_to_raw(&self) -> &dyn RawIngredient {
        // We *would* return `self` here, but this should never be executed.
        // We are never really interested in the "raw" version of an adapted ingredient.
        // Instead we use `unadapted_ingredient`, above.
        panic!("use `unadapted_ingredient` instead")
    }

    fn upcast_to_raw_mut(&mut self) -> &mut dyn RawIngredient {
        // We *would* return `self` here, but this should never be executed.
        // We are never really interested in the "raw" version of an adapted ingredient.
        // Instead we use `unadapted_ingredient`, above.
        panic!("use `unadapted_ingredient_mut` instead")
    }
}

trait AdaptedIngredientTrait: Ingredient {
    fn unadapted_ingredient(&self) -> &dyn RawIngredient;

    fn unadapted_ingredient_mut(&mut self) -> &mut dyn RawIngredient;
}

/// The adaptation shim we use to add indirection.
/// Given a `DbView`, implements `Ingredient` for any `Db: DatabaseView<DbView>`.
struct AdaptedIngredientImpl<Db, DbView: ?Sized + Any> {
    ingredient: Box<dyn Ingredient<DbView = DbView>>,
    phantom: PhantomData<Db>,
}

impl<Db, DbView: ?Sized + Any> AdaptedIngredientImpl<Db, DbView>
where
    Db: DatabaseView<DbView>,
{
    fn new(ingredient: Box<dyn Ingredient<DbView = DbView>>) -> Self {
        Self {
            ingredient,
            phantom: PhantomData,
        }
    }
}

impl<Db, DbView: ?Sized + Any> AdaptedIngredientTrait for AdaptedIngredientImpl<Db, DbView>
where
    Db: DatabaseView<DbView>,
{
    fn unadapted_ingredient(&self) -> &dyn RawIngredient {
        self.ingredient.upcast_to_raw()
    }

    fn unadapted_ingredient_mut(&mut self) -> &mut dyn RawIngredient {
        self.ingredient.upcast_to_raw_mut()
    }
}

impl<Db, DbView: ?Sized + Any> Ingredient for AdaptedIngredientImpl<Db, DbView>
where
    Db: DatabaseView<DbView>,
{
    type DbView = Db;

    fn upcast_to_raw(&self) -> &dyn RawIngredient {
        // We *would* return `self` here, but this should never be executed.
        // We are never really interested in the "raw" version of an adapted ingredient.
        // Instead we use `unadapted_ingredient`, above.
        panic!("use `unadapted_ingredient` instead")
    }

    fn upcast_to_raw_mut(&mut self) -> &mut dyn RawIngredient {
        // We *would* return `self` here, but this should never be executed.
        // We are never really interested in the "raw" version of an adapted ingredient.
        // Instead we use `unadapted_ingredient`, above.
        panic!("use `unadapted_ingredient_mut` instead")
    }

    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient.ingredient_index()
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        self.ingredient.cycle_recovery_strategy()
    }

    fn maybe_changed_after<'db>(
        &'db self,
        db: &'db Db,
        input: DependencyIndex,
        revision: Revision,
    ) -> bool {
        self.ingredient
            .maybe_changed_after(db.as_dyn(), input, revision)
    }

    fn origin(&self, key_index: Id) -> Option<QueryOrigin> {
        self.ingredient.origin(key_index)
    }

    fn mark_validated_output<'db>(
        &'db self,
        db: &'db Db,
        executor: DatabaseKeyIndex,
        output_key: Option<Id>,
    ) {
        self.ingredient
            .mark_validated_output(db.as_dyn(), executor, output_key)
    }

    fn remove_stale_output(
        &self,
        db: &Db,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<Id>,
    ) {
        self.ingredient
            .remove_stale_output(db.as_dyn(), executor, stale_output_key)
    }

    fn salsa_struct_deleted(&self, db: &Self::DbView, id: Id) {
        self.ingredient.salsa_struct_deleted(db.as_dyn(), id)
    }

    fn reset_for_new_revision(&mut self) {
        self.ingredient.reset_for_new_revision()
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.ingredient.fmt_index(index, fmt)
    }
}
