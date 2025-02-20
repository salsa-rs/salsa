use core::fmt;

use crate::{
    cycle::CycleRecoveryStrategy, ingredient::MaybeChangedAfter, zalsa::IngredientIndex, Database,
    Id,
};

// ANCHOR: DatabaseKeyIndex
/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track input and output dependencies between queries. Fully
/// ordered and equatable but those orderings are arbitrary, and meant to be used
/// only for inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DatabaseKeyIndex {
    ingredient_index: IngredientIndex,
    key_index: Id,
}
// ANCHOR_END: DatabaseKeyIndex

impl DatabaseKeyIndex {
    pub(crate) fn new(ingredient_index: IngredientIndex, key_index: Id) -> Self {
        Self {
            key_index,
            ingredient_index,
        }
    }

    pub fn ingredient_index(self) -> IngredientIndex {
        self.ingredient_index
    }

    pub fn key_index(self) -> Id {
        self.key_index
    }

    pub(crate) fn maybe_changed_after(
        &self,
        db: &dyn Database,
        last_verified_at: crate::Revision,
    ) -> MaybeChangedAfter {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .maybe_changed_after(db, self.key_index, last_verified_at)
    }

    pub(crate) fn cycle_recovery_strategy(self, db: &dyn Database) -> CycleRecoveryStrategy {
        self.ingredient_index.cycle_recovery_strategy(db)
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
}

impl fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
