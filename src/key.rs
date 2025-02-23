use core::fmt;

use crate::{
    accumulator::accumulated_map::InputAccumulatedValues,
    cycle::CycleRecoveryStrategy,
    ingredient::MaybeChangedAfter,
    zalsa::{IngredientIndex, Zalsa},
    Database, Id,
};

/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track output dependencies between queries. Fully ordered and
/// equatable but those orderings are arbitrary, and meant to be used only for
/// inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct OutputDependencyIndex {
    ingredient_index: IngredientIndex,
    key_index: Id,
}

/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track input dependencies between queries. Fully ordered and
/// equatable but those orderings are arbitrary, and meant to be used only for
/// inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct InputDependencyIndex {
    ingredient_index: IngredientIndex,
    key_index: Option<Id>,
}

impl OutputDependencyIndex {
    pub(crate) fn new(ingredient_index: IngredientIndex, key_index: Id) -> Self {
        Self {
            ingredient_index,
            key_index,
        }
    }

    pub(crate) fn remove_stale_output(
        &self,
        zalsa: &Zalsa,
        db: &dyn Database,
        executor: DatabaseKeyIndex,
    ) {
        zalsa
            .lookup_ingredient(self.ingredient_index)
            .remove_stale_output(db, executor, self.key_index)
    }

    pub(crate) fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        db: &dyn Database,
        database_key_index: DatabaseKeyIndex,
    ) {
        zalsa
            .lookup_ingredient(self.ingredient_index)
            .mark_validated_output(db, database_key_index, self.key_index)
    }
}

impl fmt::Debug for OutputDependencyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index);
            ingredient.fmt_index(Some(self.key_index), f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("OutputDependencyIndex")
                .field(&self.ingredient_index)
                .field(&self.key_index)
                .finish()
        })
    }
}

impl InputDependencyIndex {
    /// Create a database-key-index for an interning or entity table.
    /// The `key_index` here is always `None`, which deliberately corresponds to
    /// no particular id or entry. This is because the data in such tables
    /// remains valid until the table as a whole is reset. Using a single id avoids
    /// creating tons of dependencies in the dependency listings.
    pub(crate) fn for_table(ingredient_index: IngredientIndex) -> Self {
        Self {
            ingredient_index,
            key_index: None,
        }
    }

    pub(crate) fn new(ingredient_index: IngredientIndex, key_index: Id) -> Self {
        Self {
            ingredient_index,
            key_index: Some(key_index),
        }
    }

    pub(crate) fn maybe_changed_after(
        &self,
        db: &dyn Database,
        last_verified_at: crate::Revision,
    ) -> MaybeChangedAfter {
        match self.key_index {
            // SAFETY: The `db` belongs to the ingredient
            Some(key_index) => unsafe {
                db.zalsa()
                    .lookup_ingredient(self.ingredient_index)
                    .maybe_changed_after(db, key_index, last_verified_at)
            },
            // Data in tables themselves remain valid until the table as a whole is reset.
            None => MaybeChangedAfter::No(InputAccumulatedValues::Empty),
        }
    }

    pub fn set_key_index(&mut self, key_index: Id) {
        self.key_index = Some(key_index);
    }
}

impl fmt::Debug for InputDependencyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index);
            ingredient.fmt_index(self.key_index, f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("InputDependencyIndex")
                .field(&self.ingredient_index)
                .field(&self.key_index)
                .finish()
        })
    }
}

// ANCHOR: DatabaseKeyIndex
/// An "active" database key index represents a database key index
/// that is actively executing. In that case, the `key_index` cannot be
/// None.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
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
}

impl std::fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index);
            ingredient.fmt_index(Some(self.key_index), f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("DatabaseKeyIndex")
                .field(&self.ingredient_index)
                .field(&self.key_index)
                .finish()
        })
    }
}

impl From<DatabaseKeyIndex> for InputDependencyIndex {
    fn from(value: DatabaseKeyIndex) -> Self {
        Self {
            ingredient_index: value.ingredient_index,
            key_index: Some(value.key_index),
        }
    }
}

impl From<DatabaseKeyIndex> for OutputDependencyIndex {
    fn from(value: DatabaseKeyIndex) -> Self {
        Self {
            ingredient_index: value.ingredient_index,
            key_index: value.key_index,
        }
    }
}

impl TryFrom<InputDependencyIndex> for DatabaseKeyIndex {
    type Error = ();

    fn try_from(value: InputDependencyIndex) -> Result<Self, Self::Error> {
        let key_index = value.key_index.ok_or(())?;
        Ok(Self {
            ingredient_index: value.ingredient_index,
            key_index,
        })
    }
}
