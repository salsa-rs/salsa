//! User-implementable salsa traits

use std::ops::Deref;

pub trait SalsaAsRef {
    type AsRef<'a> where Self: 'a;

    fn as_ref(&self) -> Self::AsRef<'_>;
}

impl<T> SalsaAsRef for Option<T> {
    type AsRef<'a> = Option<&'a T> where Self: 'a;

    fn as_ref(&self) -> Self::AsRef<'_> {
        self.as_ref()
    }
}

impl<T, E> SalsaAsRef for Result<T, E> {
    type AsRef<'a> = Result<&'a T, &'a E> where Self: 'a;

    fn as_ref(&self) -> Self::AsRef<'_> {
        self.as_ref()
    }
}

pub trait SalsaAsDeref {
    type AsDeref<'a> where Self: 'a;

    fn as_ref(&self) -> Self::AsDeref<'_>;
}

impl<T: Deref> SalsaAsDeref for Option<T> {
    type AsDeref<'a> = Option<&'a T::Target> where Self: 'a;

    fn as_ref(&self) -> Self::AsDeref<'_> {
        self.as_deref()
    }
}

impl<T: Deref, E> SalsaAsDeref for Result<T, E> {
    type AsDeref<'a> = Result<&'a T::Target, &'a E> where Self: 'a;

    fn as_ref(&self) -> Self::AsDeref<'_> {
        self.as_deref()
    }
}
