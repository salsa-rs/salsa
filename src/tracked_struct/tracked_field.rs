use crate::{
    id::AsId, ingredient::Ingredient, key::DependencyIndex, zalsa::IngredientIndex, Database, Id,
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
pub struct FieldIngredientImpl<C>
where
    C: Configuration,
{
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,
    field_index: usize,
    struct_map: StructMapView<C>,
}

impl<C> FieldIngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn new(
        struct_index: IngredientIndex,
        field_index: usize,
        struct_map: &StructMapView<C>,
    ) -> Self {
        Self {
            ingredient_index: struct_index.successor(field_index),
            field_index,
            struct_map: struct_map.clone(),
        }
    }

    unsafe fn to_self_ref<'db>(&'db self, fields: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }

    /// Access to this value field.
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responible for selecting the appropriate element.
    pub fn field<'db>(&'db self, db: &'db dyn Database, id: Id) -> &'db C::Fields<'db> {
        let zalsa_local = db.zalsa_local();
        let current_revision = db.zalsa().current_revision();
        let data = self.struct_map.get(current_revision, id);
        let data = C::deref_struct(data);
        let changed_at = data.revisions[self.field_index];

        zalsa_local.report_tracked_read(
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

impl<C> Ingredient for FieldIngredientImpl<C>
where
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
        db: &'db dyn Database,
        input: Option<Id>,
        revision: crate::Revision,
    ) -> bool {
        let id = input.unwrap();
        let data = self
            .struct_map
            .get_and_validate_last_changed(db.zalsa(), id);
        let data = C::deref_struct(data);
        let field_changed_at = data.revisions[self.field_index];
        field_changed_at > revision
    }

    fn origin(&self, _key_index: crate::Id) -> Option<crate::zalsa_local::QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        _executor: crate::DatabaseKeyIndex,
        _output_key: Option<crate::Id>,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        _executor: crate::DatabaseKeyIndex,
        _stale_output_key: Option<crate::Id>,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn salsa_struct_deleted(&self, _db: &dyn Database, _id: crate::Id) {
        panic!("tracked field ingredients are not registered as dependent")
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
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
            C::DEBUG_NAME,
            C::FIELD_DEBUG_NAMES[self.field_index],
            index.unwrap()
        )
    }

    fn debug_name(&self) -> &'static str {
        C::FIELD_DEBUG_NAMES[self.field_index]
    }
}

impl<C> std::fmt::Debug for FieldIngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("ingredient_index", &self.ingredient_index)
            .field("field_index", &self.field_index)
            .finish()
    }
}
