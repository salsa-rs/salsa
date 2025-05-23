use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU64;

use crate::zalsa::Zalsa;

/// The `Id` of a salsa struct in the database [`Table`](`crate::table::Table`).
///
/// The high-order bits of an `Id` store a 32-bit generation counter, while
/// the low-order bits pack a [`PageIndex`](`crate::table::PageIndex`) and
/// [`SlotIndex`](`crate::table::SlotIndex`) within the page.
///
/// The low-order bits of `Id` are a `u32` ranging from `0..Id::MAX_U32`.
/// The maximum range is smaller than a standard `u32` to leave
/// room for niches; currently there is only one niche, so that
/// `Option<Id>` is the same size as an `Id`.
///
/// As an end-user of `Salsa` you will generally not use `Id` directly,
/// it is wrapped in new types.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    value: NonZeroU64,
}

impl Id {
    pub const MAX_U32: u32 = u32::MAX - 0xFF;
    pub const MAX_USIZE: usize = Self::MAX_U32 as usize;

    /// Create a `salsa::Id` from a u32 value, without a generation. This
    /// value should be less than [`Self::MAX_U32`].
    ///
    /// In general, you should not need to create salsa ids yourself,
    /// but it can be useful if you are using the type as a general
    /// purpose "identifier" internally.
    ///
    /// # Safety
    ///
    /// The supplied value must be less than [`Self::MAX_U32`].
    #[doc(hidden)]
    #[track_caller]
    #[inline]
    pub const unsafe fn from_index(v: u32) -> Self {
        debug_assert!(v < Self::MAX_U32);

        Id {
            // SAFETY: Caller obligation.
            value: unsafe { NonZeroU64::new_unchecked((v + 1) as u64) },
        }
    }

    /// Create a `salsa::Id` from a u64 value.
    ///
    /// This should only be used to recreate an `Id` together with `Id::as_u64`.
    ///
    /// # Safety
    ///
    /// The data bits of the supplied value must represent a valid `Id` returned
    /// by `Id::as_u64`.
    #[doc(hidden)]
    #[track_caller]
    #[inline]
    pub const unsafe fn from_bits(v: u64) -> Self {
        Id {
            // SAFETY: Caller obligation.
            value: unsafe { NonZeroU64::new_unchecked(v) },
        }
    }

    /// Returns a new `Id` with same index, but the generation incremented by one.
    ///
    /// Returns `None` if the generation would overflow, i.e. the current generation
    /// is `u32::MAX`.
    #[inline]
    pub fn next_generation(self) -> Option<Id> {
        self.generation()
            .checked_add(1)
            .map(|generation| self.with_generation(generation))
    }

    /// Mark the `Id` with a generation.
    ///
    /// This `Id` will refer to the same page and slot in the database,
    /// but will differ from other identifiers of the slot based on the
    /// provided generation.
    #[inline]
    pub const fn with_generation(self, generation: u32) -> Id {
        let mut value = self.value.get();

        value &= 0xFFFFFFFF;
        value |= (generation as u64) << 32;

        Id {
            // SAFETY: The niche of `value` is in the lower bits, which we did not touch.
            value: unsafe { NonZeroU64::new_unchecked(value) },
        }
    }

    /// Return the index portion of this `Id`.
    #[inline]
    pub const fn index(self) -> u32 {
        // Truncate the high-order bits.
        (self.value.get() as u32) - 1
    }

    /// Return the generation of this `Id`.
    #[inline]
    pub const fn generation(self) -> u32 {
        // Shift away the low-order bits.
        (self.value.get() >> 32) as u32
    }

    /// Return the internal `u64` representation of this `Id`.
    #[inline]
    pub const fn as_bits(self) -> u64 {
        self.value.get()
    }
}

impl Debug for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.generation() == 0 {
            write!(f, "Id({:x})", self.index())
        } else {
            write!(f, "Id({:x}g{:x})", self.index(), self.generation())
        }
    }
}

/// Internal salsa trait for types that can be represented as a salsa id.
pub trait AsId: Sized {
    fn as_id(&self) -> Id;
}

/// Internal Salsa trait for types that are just a newtype'd [`Id`][].
pub trait FromId {
    fn from_id(id: Id) -> Self;
}

impl AsId for Id {
    #[inline]
    fn as_id(&self) -> Id {
        *self
    }
}

impl FromId for Id {
    #[inline]
    fn from_id(id: Id) -> Self {
        id
    }
}

/// Enums cannot use [`FromId`] because they need access to the DB to tell the `TypeId` of the variant,
/// so they use this trait instead, that has a blanket implementation for `FromId`.
pub trait FromIdWithDb {
    fn from_id(id: Id, zalsa: &Zalsa) -> Self;
}

impl<T: FromId> FromIdWithDb for T {
    #[inline]
    fn from_id(id: Id, _zalsa: &Zalsa) -> Self {
        FromId::from_id(id)
    }
}
