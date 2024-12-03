use crate::{
    accumulator::accumulated_map::AccumulatedMap, cycle::CycleRecoveryStrategy,
    zalsa::IngredientIndex, Database, Id,
};

// ANCHOR: DatabaseKeyIndex
/// An "active" database key index represents a database key index
/// that is actively executing. In that case, the `key_index` cannot be
/// None.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DatabaseKeyIndex {
    pub(crate) ingredient_index: IngredientIndex,
    pub(crate) key_index: Id,
}
// ANCHOR_END: DatabaseKeyIndex

impl DatabaseKeyIndex {
    pub fn ingredient_index(self) -> IngredientIndex {
        self.ingredient_index
    }

    pub fn key_index(self) -> Id {
        self.key_index
    }

    pub(crate) fn cycle_recovery_strategy(self, db: &dyn Database) -> CycleRecoveryStrategy {
        self.ingredient_index.cycle_recovery_strategy(db)
    }

    pub(crate) fn accumulated(self, db: &dyn Database) -> Option<&AccumulatedMap> {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .accumulated(db, self.key_index)
    }

    pub(crate) fn remove_stale_output(&self, db: &dyn Database, executor: DatabaseKeyIndex) {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .remove_stale_output(db, executor, self.key_index)
    }

    pub(crate) fn mark_validated_output(
        &self,
        db: &dyn Database,
        database_key_index: DatabaseKeyIndex,
    ) {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .mark_validated_output(db, database_key_index, self.key_index)
    }

    pub(crate) fn maybe_changed_after(
        &self,
        db: &dyn Database,
        last_verified_at: crate::Revision,
    ) -> bool {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .maybe_changed_after(db, self.key_index, last_verified_at)
    }
}

impl std::fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index);
            ingredient.fmt_index(self.key_index, f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("DatabaseKeyIndex")
                .field(&self.ingredient_index)
                .field(&self.key_index)
                .finish()
        })
    }
}
