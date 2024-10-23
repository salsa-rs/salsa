use crate::{accumulator::accumulated_map::AccumulatedMap, zalsa::IngredientIndex, Database, Id};

/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track dependencies between queries. Fully ordered and
/// equatable but those orderings are arbitrary, and meant to be used only for
/// inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DependencyIndex {
    pub(crate) ingredient_index: IngredientIndex,
    pub(crate) key_index: Option<Id>,
}

impl DependencyIndex {
    /// Create a database-key-index for an interning or entity table.
    /// The `key_index` here is always zero, which deliberately corresponds to
    /// no particular id or entry. This is because the data in such tables
    /// remains valid until the table as a whole is reset. Using a single id avoids
    /// creating tons of dependencies in the dependency listings.
    pub(crate) fn for_table(ingredient_index: IngredientIndex) -> Self {
        Self {
            ingredient_index,
            key_index: None,
        }
    }

    pub fn ingredient_index(self) -> IngredientIndex {
        self.ingredient_index
    }

    pub fn key_index(self) -> Option<Id> {
        self.key_index
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

impl std::fmt::Debug for DependencyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index);
            ingredient.fmt_index(self.key_index, f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("DependencyIndex")
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

    pub(crate) fn accumulated(self, db: &dyn Database) -> Option<&AccumulatedMap> {
        db.zalsa()
            .lookup_ingredient(self.ingredient_index)
            .accumulated(db, self.key_index)
    }
}

impl std::fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let i: DependencyIndex = (*self).into();
        std::fmt::Debug::fmt(&i, f)
    }
}

impl From<DatabaseKeyIndex> for DependencyIndex {
    fn from(value: DatabaseKeyIndex) -> Self {
        Self {
            ingredient_index: value.ingredient_index,
            key_index: Some(value.key_index),
        }
    }
}

impl TryFrom<DependencyIndex> for DatabaseKeyIndex {
    type Error = ();

    fn try_from(value: DependencyIndex) -> Result<Self, Self::Error> {
        let key_index = value.key_index.ok_or(())?;
        Ok(Self {
            ingredient_index: value.ingredient_index,
            key_index,
        })
    }
}
