use std::fmt;
use std::num::NonZeroU32;

use crate::Id;
use crate::function::VerifyResult;
use crate::zalsa::{IngredientIndex, Zalsa};

// ANCHOR: DatabaseKeyIndex
/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track input and output dependencies between queries. Fully
/// ordered and equatable but those orderings are arbitrary, and meant to be used
/// only for inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DatabaseKeyIndex {
    index: NonZeroU32,
    metadata: u32,
}
// ANCHOR_END: DatabaseKeyIndex

impl DatabaseKeyIndex {
    const INGREDIENT_SHIFT: u32 = 20;
    const TAG_SHIFT: u32 = 31;
    const GENERATION_MASK: u32 = Id::MAX_GENERATION;
    const INGREDIENT_MASK: u32 = 0x7FF;

    #[inline]
    pub(crate) const fn new(ingredient_index: IngredientIndex, key_index: Id) -> Self {
        let ingredient = ingredient_index.with_tag(false).as_u32();
        let tag = ingredient_index.tag();
        let generation = key_index.generation();

        assert!(ingredient <= Self::INGREDIENT_MASK);
        assert!(generation <= Self::GENERATION_MASK);

        Self {
            // SAFETY: A valid `Id` index is at most `Id::MAX_U32`, so adding one is non-zero
            // and does not overflow.
            index: unsafe { NonZeroU32::new_unchecked(key_index.index() + 1) },
            metadata: generation
                | (ingredient << Self::INGREDIENT_SHIFT)
                | ((tag as u32) << Self::TAG_SHIFT),
        }
    }

    pub const fn ingredient_index(self) -> IngredientIndex {
        let ingredient = (self.metadata >> Self::INGREDIENT_SHIFT) & Self::INGREDIENT_MASK;
        let tag = self.metadata >> Self::TAG_SHIFT != 0;

        // SAFETY: The 12 ingredient bits were initialized from a valid `IngredientIndex`.
        unsafe { IngredientIndex::new_unchecked(ingredient) }.with_tag(tag)
    }

    pub const fn key_index(self) -> Id {
        // SAFETY: `index` was initialized from a valid `Id`.
        unsafe { Id::from_index(self.index.get() - 1) }
            .with_generation(self.metadata & Self::GENERATION_MASK)
    }

    pub(crate) fn maybe_changed_after(
        &self,
        db: crate::database::RawDatabase<'_>,
        zalsa: &Zalsa,
        last_verified_at: crate::Revision,
    ) -> VerifyResult {
        // SAFETY: The `db` belongs to the ingredient
        unsafe {
            // here, `db` has to be either the correct type already, or a subtype (as far as trait
            // hierarchy is concerned)
            zalsa
                .lookup_ingredient(self.ingredient_index())
                .maybe_changed_after(zalsa, db, self.key_index(), last_verified_at)
        }
    }

    pub(crate) fn remove_stale_output(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        zalsa
            .lookup_ingredient(self.ingredient_index())
            .remove_stale_output(zalsa, executor, self.key_index())
    }

    pub(crate) fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) {
        zalsa
            .lookup_ingredient(self.ingredient_index())
            .mark_validated_output(zalsa, database_key_index, self.key_index())
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for DatabaseKeyIndex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&(self.key_index(), self.ingredient_index()), serializer)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for DatabaseKeyIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (key_index, ingredient_index) = serde::Deserialize::deserialize(deserializer)?;

        Ok(DatabaseKeyIndex::new(ingredient_index, key_index))
    }
}

const _: [(); 8] = [(); std::mem::size_of::<DatabaseKeyIndex>()];
const _: [(); 8] = [(); std::mem::size_of::<Option<DatabaseKeyIndex>>()];

#[cfg(test)]
mod tests {
    use super::DatabaseKeyIndex;
    use crate::{Id, IngredientIndex};

    #[test]
    fn round_trip_largest_supported_values() {
        let id = unsafe { Id::from_index(Id::MAX_U32 - 1) }
            .with_generation(DatabaseKeyIndex::GENERATION_MASK);
        let key =
            DatabaseKeyIndex::new(IngredientIndex::new(DatabaseKeyIndex::INGREDIENT_MASK), id);

        assert_eq!(key.ingredient_index().as_u32(), 0x7FF);
        assert_eq!(key.key_index(), id);
    }

    #[test]
    fn round_trip_tagged_ingredient() {
        let id = unsafe { Id::from_index(0) };
        let ingredient = IngredientIndex::new(42).with_tag(true);
        let key = DatabaseKeyIndex::new(ingredient, id);

        assert_eq!(key.ingredient_index(), ingredient);
    }

    #[test]
    #[should_panic]
    fn rejects_generation_overflow() {
        let id =
            unsafe { Id::from_index(0) }.with_generation(DatabaseKeyIndex::GENERATION_MASK + 1);
        DatabaseKeyIndex::new(IngredientIndex::new(0), id);
    }
}

impl fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::attach::with_attached_database(|db| {
            let ingredient = db.zalsa().lookup_ingredient(self.ingredient_index());
            ingredient.fmt_index(self.key_index(), f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("DatabaseKeyIndex")
                .field(&self.ingredient_index())
                .field(&self.key_index())
                .finish()
        })
    }
}
