use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU32;

use crate::Database;

/// An Id is a newtype'd u32 ranging from `0..Id::MAX_U32`.
/// The maximum range is smaller than a standard u32 to leave
/// room for niches; currently there is only one niche, so that
/// `Option<Id>` is the same size as an `Id`.
///
/// You will rarely use the `Id` type directly, though you can.
/// You are more likely to use types that implement the `AsId` trait,
/// such as entity keys.
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
        write!(f, "Id({})", self.as_u32())
    }
}

impl From<u32> for Id {
    fn from(n: u32) -> Self {
        Id::from_u32(n)
    }
}

impl From<usize> for Id {
    fn from(n: usize) -> Self {
        assert!(n < Id::MAX_USIZE);
        Id::from_u32(n as u32)
    }
}

impl From<Id> for u32 {
    fn from(n: Id) -> Self {
        n.as_u32()
    }
}

impl From<Id> for usize {
    fn from(n: Id) -> usize {
        n.as_u32() as usize
    }
}

/// Internal salsa trait for types that can be represented as a salsa id.
pub trait AsId: Sized {
    fn as_id(&self) -> Id;
}

/// Internal Salsa trait for types that have a salsa id but require looking
/// up in the database to find it. This is different from
/// [`AsId`][] where what we have is literally a *newtype*
/// for an `Id`.
pub trait LookupId<'db>: AsId {
    /// Lookup from an `Id` to get an instance of the type.
    ///
    /// # Panics
    ///
    /// This fn may panic if the value with this id has not been
    /// produced in this revision already (e.g., for a tracked
    /// struct, the function will panic if the tracked struct
    /// has not yet been created in this revision). Salsa's
    /// dependency tracking typically ensures this does not
    /// occur, but it is possible for a user to violate this
    /// rule.
    fn lookup_id(id: Id, db: &'db dyn Database) -> Self;
}

/// Internal Salsa trait for types that are just a newtype'd [`Id`][].
pub trait FromId: AsId + Copy + Eq + Hash + Debug {
    fn from_id(id: Id) -> Self;

    fn from_as_id(id: &impl AsId) -> Self {
        Self::from_id(id.as_id())
    }
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

/// As a special case, we permit `Singleton` to be converted to an `Id`.
/// This is useful for declaring functions with no arguments.
impl AsId for () {
    fn as_id(&self) -> Id {
        Id::from_u32(0)
    }
}

impl FromId for () {
    fn from_id(id: Id) -> Self {
        assert_eq!(0, id.as_u32());
    }
}

impl<'db, ID: FromId> LookupId<'db> for ID {
    fn lookup_id(id: Id, _db: &'db dyn Database) -> Self {
        Self::from_id(id)
    }
}
