use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU32;

/// An Id is a newtype'd u32 ranging from `0..Id::MAX_U32`.
/// The maximum range is smaller than a standard u32 to leave
/// room for niches; currently there is only one niche, so that
/// `Option<Id>` is the same size as an `Id`.
///
/// You will rarely use the `Id` type directly, though you can.
/// You are more likely to use types that implement the `AsId` trait,
/// such as entity keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Id {
    value: NonZeroU32,
}

impl Id {
    pub const MAX_U32: u32 = std::u32::MAX - 0xFF;
    pub const MAX_USIZE: usize = Self::MAX_U32 as usize;

    /// Create a `salsa::Id` from a u32 value. This value should
    /// be less than [`Self::MAX_U32`].
    ///
    /// In general, you should not need to create salsa ids yourself,
    /// but it can be useful if you are using the type as a general
    /// purpose "identifier" internally.
    #[track_caller]
    pub fn from_u32(x: u32) -> Self {
        assert!(x < Self::MAX_U32);
        Id {
            value: NonZeroU32::new(x + 1).unwrap(),
        }
    }

    pub fn as_u32(self) -> u32 {
        self.value.get() - 1
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

/// Trait for types that can be interconverted to a salsa Id;
pub trait AsId: Sized + Copy + Eq + Hash + Debug {
    fn as_id(self) -> Id;
    fn from_id(id: Id) -> Self;
}

impl AsId for Id {
    fn as_id(self) -> Id {
        self
    }

    fn from_id(id: Id) -> Self {
        id
    }
}

/// As a special case, we permit `()` to be converted to an `Id`.
/// This is useful for declaring functions with no arguments.
impl AsId for () {
    fn as_id(self) -> Id {
        Id::from_u32(0)
    }

    fn from_id(id: Id) -> Self {
        assert_eq!(0, id.as_u32());
    }
}
