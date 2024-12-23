use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU32;

/// The `Id` of a salsa struct in the database [`Table`](`crate::table::Table`).
///
/// The higher-order bits of an `Id` identify a [`Page`](`crate::table::Page`)
/// and the low-order bits identify a slot within the page.
///
/// An Id is a newtype'd u32 ranging from `0..Id::MAX_U32`.
/// The maximum range is smaller than a standard u32 to leave
/// room for niches; currently there is only one niche, so that
/// `Option<Id>` is the same size as an `Id`.
///
/// As an end-user of `Salsa` you will not use `Id` directly,
/// it is wrapped in new types.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    value: NonZeroU32,
}

impl Id {
    pub const MAX_U32: u32 = u32::MAX - 0xFF;
    pub const MAX_USIZE: usize = Self::MAX_U32 as usize;

    /// Create a `salsa::Id` from a u32 value. This value should
    /// be less than [`Self::MAX_U32`].
    ///
    /// In general, you should not need to create salsa ids yourself,
    /// but it can be useful if you are using the type as a general
    /// purpose "identifier" internally.
    #[doc(hidden)]
    #[track_caller]
    pub const fn from_u32(x: u32) -> Self {
        Id {
            value: match NonZeroU32::new(x + 1) {
                Some(v) => v,
                None => panic!("given value is too large to be a `salsa::Id`"),
            },
        }
    }

    pub const fn as_u32(self) -> u32 {
        self.value.get() - 1
    }
}

impl Debug for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Id({:x})", self.as_u32())
    }
}

/// Internal salsa trait for types that can be represented as a salsa id.
pub trait AsId: Sized {
    fn as_id(&self) -> Id;
}

/// Internal Salsa trait for types that are just a newtype'd [`Id`][].
pub trait FromId: AsId + Copy + Eq + Hash + Debug {
    fn from_id(id: Id) -> Self;
}

impl AsId for Id {
    fn as_id(&self) -> Id {
        *self
    }
}

impl FromId for Id {
    fn from_id(id: Id) -> Self {
        id
    }
}
