use crate::{
    id::AsId,
    ingredient::{Ingredient, IngredientRequiresReset},
    key::DependencyIndex,
    Database, Id, IngredientIndex, Runtime,
};

use super::{struct_map::StructMapView, Configuration};

/// Created for each tracked struct.
/// This ingredient only stores the "id" fields.
/// It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked struct id.
/// The value fields are stored in [`crate::function::FunctionIngredient`] instances keyed by the tracked struct id.
/// Unlike normal interners, tracked struct indices can be deleted and reused aggressively:
/// when a tracked function re-executes,
/// any tracked structs that it created before but did not create this time can be deleted.
pub struct TrackedFieldIngredient<C>
where
    C: Configuration,
{
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    pub(super) ingredient_index: IngredientIndex,
    pub(super) field_index: u32,
    pub(super) struct_map: StructMapView<C>,
    pub(super) struct_debug_name: &'static str,
    pub(super) field_debug_name: &'static str,
}

impl<C> TrackedFieldIngredient<C>
where
    C: Configuration,
{
    unsafe fn to_self_ref<'db>(&'db self, fields: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }

    /// Access to this value field.
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responible for selecting the appropriate element.
    pub fn field<'db>(&'db self, runtime: &'db Runtime, id: Id) -> &'db C::Fields<'db> {
        let data = self.struct_map.get(runtime, id);

        let changed_at = C::revision(&data.revisions, self.field_index);

        runtime.report_tracked_read(
            DependencyIndex {
                ingredient_index: self.ingredient_index,
                key_index: Some(id.as_id()),
            },
            data.durability,
            changed_at,
        );

        unsafe { self.to_self_ref(&data.fields) }
    }
}

impl<DB: ?Sized, C> Ingredient<DB> for TrackedFieldIngredient<C>
where
    DB: Database,
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn cycle_recovery_strategy(&self) -> crate::cycle::CycleRecoveryStrategy {
        crate::cycle::CycleRecoveryStrategy::Panic
    }

    fn maybe_changed_after<'db>(
        &'db self,
        db: &'db DB,
        input: crate::key::DependencyIndex,
        revision: crate::Revision,
    ) -> bool {
        let runtime = db.runtime();
        let id = input.key_index.unwrap();
        let data = self.struct_map.get(runtime, id);
        let field_changed_at = C::revision(&data.revisions, self.field_index);
        field_changed_at > revision
    }

    fn origin(&self, _key_index: crate::Id) -> Option<crate::runtime::local_state::QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &DB,
        _executor: crate::DatabaseKeyIndex,
        _output_key: Option<crate::Id>,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn remove_stale_output(
        &self,
        _db: &DB,
        _executor: crate::DatabaseKeyIndex,
        _stale_output_key: Option<crate::Id>,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn salsa_struct_deleted(&self, _db: &DB, _id: crate::Id) {
        panic!("tracked field ingredients are not registered as dependent")
    }

    fn reset_for_new_revision(&mut self) {
        panic!("tracked field ingredients do not require reset")
    }

    fn fmt_index(
        &self,
        index: Option<crate::Id>,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(
            fmt,
            "{}.{}({:?})",
            self.struct_debug_name,
            self.field_debug_name,
            index.unwrap()
        )
    }
}

impl<C> IngredientRequiresReset for TrackedFieldIngredient<C>
where
    C: Configuration,
{
    const RESET_ON_NEW_REVISION: bool = false;
}
