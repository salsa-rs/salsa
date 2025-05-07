use core::fmt;

use crate::function::VerifyResult;
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Database, Id};

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
    #[inline]
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
        zalsa: &Zalsa,
        last_verified_at: crate::Revision,
        in_cycle: bool,
    ) -> VerifyResult {
        // SAFETY: The `db` belongs to the ingredient
        unsafe {
            zalsa
                .lookup_ingredient(self.ingredient_index)
                .maybe_changed_after(db, self.key_index, last_verified_at, in_cycle)
        }
    }

    pub(crate) fn remove_stale_output(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        zalsa
            .lookup_ingredient(self.ingredient_index)
            .remove_stale_output(zalsa, executor, self.key_index)
    }

    pub(crate) fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) {
        zalsa
            .lookup_ingredient(self.ingredient_index)
            .mark_validated_output(zalsa, database_key_index, self.key_index)
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
