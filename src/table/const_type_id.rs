//! Forked from <https://github.com/dtolnay/typeid>

use core::any::TypeId;
use core::cmp::Ordering;
use core::fmt::{self, Debug};
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;

/// TypeId equivalent usable in const contexts.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ConstTypeId {
    type_id_fn: fn() -> TypeId,
}

impl ConstTypeId {
    /// Create a [`ConstTypeId`] for a type.
    #[must_use]
    pub const fn of<T>() -> Self
    where
        T: ?Sized,
    {
        ConstTypeId {
            type_id_fn: of::<T>,
        }
    }

    #[inline]
    fn get(self) -> TypeId {
        (self.type_id_fn)()
    }
}

impl Debug for ConstTypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(&self.get(), formatter)
    }
}

impl PartialEq for ConstTypeId {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl PartialEq<TypeId> for ConstTypeId {
    fn eq(&self, other: &TypeId) -> bool {
        self.get() == *other
    }
}

impl Eq for ConstTypeId {}

impl PartialOrd for ConstTypeId {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for ConstTypeId {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        Ord::cmp(&self.get(), &other.get())
    }
}

impl Hash for ConstTypeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get().hash(state);
    }
}

/// Create a [`TypeId`] for a type.
#[must_use]
#[inline(always)]
pub fn of<T>() -> TypeId
where
    T: ?Sized,
{
    trait NonStaticAny {
        fn get_type_id(&self) -> TypeId
        where
            Self: 'static;
    }

    impl<T: ?Sized> NonStaticAny for PhantomData<T> {
        #[inline(always)]
        fn get_type_id(&self) -> TypeId
        where
            Self: 'static,
        {
            TypeId::of::<T>()
        }
    }

    let phantom_data = PhantomData::<T>;
    // SAFETY: We only punt the lifetime to fetch the type id, we don't actually use the lifetime
    // in any meaningful way.
    NonStaticAny::get_type_id(unsafe {
        mem::transmute::<&dyn NonStaticAny, &(dyn NonStaticAny + 'static)>(&phantom_data)
    })
}
