use std::fmt;
use std::num::NonZeroU32;

use crate::function::{VerifyCycleHeads, VerifyResult};
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Database, Id};

// ANCHOR: DatabaseKeyIndex
/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track input and output dependencies between queries. Fully
/// ordered and equatable but those orderings are arbitrary, and meant to be used
/// only for inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DatabaseKeyIndex {
    index: NonZeroU32,
    /// `DatabaseKeyIndex` is stored *a lot*, as every query dependency stores one. Therefore
    /// we want to make it compact.
    ///
    /// The ingredient index is technically needed only for tracked fns - other things can
    /// grab it from the page, whose index is stored in `index`. On the other hand,
    /// only interned structs need a generation - for GC. So we store like the following:
    ///
    /// If this is an interned, this stores the generation (left-shifted by a bit and ORed with 0b1,
    /// as every generation is). If not, the LSB is 0, and the rest of the bits store the
    /// `IngredientIndex` shifted left by 1 bit.
    generation_or_ingredient_index: u32,
}
// ANCHOR_END: DatabaseKeyIndex

impl DatabaseKeyIndex {
    const INGREDIENT_INDEX_SHIFT: u32 = 2;

    #[inline]
    pub(crate) const fn new_interned(_ingredient_index: IngredientIndex, key_index: Id) -> Self {
        Self {
            index: key_index.index_nonzero(),
            generation_or_ingredient_index: key_index.generation(),
        }
    }

    #[inline]
    pub(crate) const fn new_non_interned(ingredient_index: IngredientIndex, key_index: Id) -> Self {
        Self {
            index: key_index.index_nonzero(),
            generation_or_ingredient_index: ingredient_index.as_u32()
                << Self::INGREDIENT_INDEX_SHIFT,
        }
    }

    #[inline]
    pub(crate) fn new_non_interned_with_tag(
        ingredient_index: IngredientIndex,
        key_index: Id,
    ) -> Self {
        let mut result = Self::new_non_interned(ingredient_index, key_index);
        result.generation_or_ingredient_index |= 0b10;
        result
    }

    #[inline]
    pub(crate) fn has_tag(self) -> bool {
        // The LSB is 0, meaning a non-interned, and the second LSB is 1, meaning the tag is on.
        (self.generation_or_ingredient_index & 0b11) == 0b10
    }

    #[inline]
    pub(crate) fn ingredient_index_with_zalsa(self, zalsa: &Zalsa) -> IngredientIndex {
        if self.is_interned() {
            zalsa.ingredient_index(self.key_index())
        } else {
            IngredientIndex::new(
                self.generation_or_ingredient_index >> Self::INGREDIENT_INDEX_SHIFT,
            )
        }
    }

    #[inline]
    pub fn ingredient_index(self, db: &dyn Database) -> IngredientIndex {
        self.ingredient_index_with_zalsa(db.zalsa())
    }

    #[track_caller]
    #[cfg(feature = "persistence")]
    pub(crate) fn ingredient_index_assert_non_interned(self) -> IngredientIndex {
        assert!(!self.is_interned());
        IngredientIndex::new(self.generation_or_ingredient_index >> Self::INGREDIENT_INDEX_SHIFT)
    }

    /// **Warning:** For tracked fns on interned structs, the generation is *permanently lost* once stored
    /// in a `DatabaseKeyIndex`. Usually it is not a problem to bring an accurate [`Id`] from a different place,
    /// and where it is hard, it is not needed. However you need to be careful when handling this.
    pub fn key_index(self) -> Id {
        let generation = if self.is_interned() {
            self.generation_or_ingredient_index
        } else {
            1
        };
        Id::from_raw_parts(self.index, generation)
    }

    #[inline]
    fn is_interned(self) -> bool {
        (self.generation_or_ingredient_index & 0b1) == 0b1
    }

    pub(crate) fn maybe_changed_after(
        &self,
        db: crate::database::RawDatabase<'_>,
        zalsa: &Zalsa,
        last_verified_at: crate::Revision,
        cycle_heads: &mut VerifyCycleHeads,
    ) -> VerifyResult {
        // SAFETY: The `db` belongs to the ingredient
        unsafe {
            // here, `db` has to be either the correct type already, or a subtype (as far as trait
            // hierarchy is concerned)
            zalsa
                .lookup_ingredient(self.ingredient_index_with_zalsa(zalsa))
                .maybe_changed_after(zalsa, db, self.key_index(), last_verified_at, cycle_heads)
        }
    }

    pub(crate) fn remove_stale_output(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        zalsa
            .lookup_ingredient(self.ingredient_index_with_zalsa(zalsa))
            .remove_stale_output(zalsa, executor, self.key_index())
    }

    pub(crate) fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) {
        zalsa
            .lookup_ingredient(self.ingredient_index_with_zalsa(zalsa))
            .mark_validated_output(zalsa, database_key_index, self.key_index())
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for DatabaseKeyIndex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(
            &(self.index, self.generation_or_ingredient_index),
            serializer,
        )
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for DatabaseKeyIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (index, generation_or_ingredient_index) =
            serde::Deserialize::deserialize(deserializer)?;

        Ok(DatabaseKeyIndex {
            index,
            generation_or_ingredient_index,
        })
    }
}

impl fmt::Debug for DatabaseKeyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::attach::with_attached_database(|db| {
            let zalsa = db.zalsa();
            let ingredient = zalsa.lookup_ingredient(self.ingredient_index_with_zalsa(zalsa));
            ingredient.fmt_index(self.key_index(), f)
        })
        .unwrap_or_else(|| {
            f.debug_tuple("DatabaseKeyIndex")
                .field(&self.index)
                .field(&self.generation_or_ingredient_index)
                .finish()
        })
    }
}
