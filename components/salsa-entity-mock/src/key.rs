use std::fmt::Debug;

use crate::{Database, Id, IngredientIndex};

/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track dependencies between queries. Fully ordered and
/// equatable but those orderings are arbitrary, and meant to be used only for
/// inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
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

    pub fn debug<DB: ?Sized + Database>(self, _db: &DB) -> impl Debug + '_ {
        self // FIXME
    }
}

/// An "active" database key index represents a database key index
/// that is actively executing. In that case, the `key_index` cannot be
/// None.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DatabaseKeyIndex {
    pub(crate) ingredient_index: IngredientIndex,
    pub(crate) key_index: Id,
}

impl DatabaseKeyIndex {
    pub fn ingredient_index(self) -> IngredientIndex {
        self.ingredient_index
    }

    pub fn key_index(self) -> Id {
        self.key_index
    }

    pub fn debug<DB: ?Sized + Database>(self, db: &DB) -> impl Debug + '_ {
        let i: DependencyIndex = self.into();
        i.debug(db)
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
