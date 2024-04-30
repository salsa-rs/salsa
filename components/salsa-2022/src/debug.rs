use std::{
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
    sync::Arc,
};

use crate::database::AsSalsaDatabase;

/// `DebugWithDb` is a version of the traditional [`Debug`](`std::fmt::Debug`)
/// trait that gives access to the salsa database, allowing tracked
/// structs to print the values of their fields. It is typically not used
/// directly, instead you should write (e.g.) `format!("{:?}", foo.debug(db))`.
/// Implementations are automatically provided for `#[salsa::tracked]`
/// items, though you can opt-out from that if you wish to provide a manual
/// implementation.
///
/// # WARNING: Intended for debug use only!
///
/// Debug print-outs of tracked structs include the value of all their fields,
/// but the reads of those fields are ignored by salsa. This avoids creating
/// spurious dependencies from debugging code, but if you use the resulting
/// string to influence the outputs (return value, accumulators, etc) from your
/// query, salsa's dependency tracking will be undermined.
///
/// If for some reason you *want* to incorporate dependency output into
/// your query, do not use the `debug` or `into_debug` helpers and instead
/// invoke `fmt` manually.
pub trait DebugWithDb<Db: ?Sized + AsSalsaDatabase> {
    /// Creates a wrapper type that implements `Debug` but which
    /// uses the `DebugWithDb::fmt`.
    ///
    /// # WARNING: Intended for debug use only!
    ///
    /// The wrapper type Debug impl will access the value of all
    /// fields but those accesses are ignored by salsa. This is only
    /// suitable for debug output. See [`DebugWithDb`][] trait comment
    /// for more details.
    fn debug<'me, 'db>(&'me self, db: &'me Db) -> DebugWith<'me, Db>
    where
        Self: Sized + 'me,
    {
        DebugWith {
            value: BoxRef::Ref(self),
            db,
        }
    }

    /// Creates a wrapper type that implements `Debug` but which
    /// uses the `DebugWithDb::fmt`.
    ///
    /// # WARNING: Intended for debug use only!
    ///
    /// The wrapper type Debug impl will access the value of all
    /// fields but those accesses are ignored by salsa. This is only
    /// suitable for debug output. See [`DebugWithDb`][] trait comment
    /// for more details.
    fn into_debug<'me, 'db>(self, db: &'me Db) -> DebugWith<'me, Db>
    where
        Self: Sized + 'me,
    {
        DebugWith {
            value: BoxRef::Box(Box::new(self)),
            db,
        }
    }

    /// Format `self` given the database `db`.
    ///
    /// # Dependency tracking
    ///
    /// When invoked manually, field accesses that occur
    /// within this method are tracked by salsa. But when invoked
    /// the [`DebugWith`][] value returned by the [`debug`](`Self::debug`)
    /// and [`into_debug`][`Self::into_debug`] methods,
    /// those accesses are ignored.
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result;
}

/// Helper type for the [`DebugWithDb`][] trait that
/// wraps a value and implements [`std::fmt::Debug`][],
/// redirecting calls to the `fmt` method from [`DebugWithDb`][].
///
/// # WARNING: Intended for debug use only!
///
/// This type intentionally ignores salsa dependencies used
/// to generate the debug output. See the [`DebugWithDb`][] trait
/// for more notes on this.
pub struct DebugWith<'me, Db: ?Sized + AsSalsaDatabase> {
    value: BoxRef<'me, dyn DebugWithDb<Db> + 'me>,
    db: &'me Db,
}

enum BoxRef<'me, T: ?Sized> {
    Box(Box<T>),
    Ref(&'me T),
}

impl<T: ?Sized> std::ops::Deref for BoxRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            BoxRef::Box(b) => b,
            BoxRef::Ref(r) => r,
        }
    }
}

impl<Db: ?Sized> fmt::Debug for DebugWith<'_, Db>
where
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let db = self.db.as_salsa_database();
        db.runtime()
            .debug_probe(|| DebugWithDb::fmt(&*self.value, f, self.db))
    }
}

impl<Db: ?Sized, T: ?Sized> DebugWithDb<Db> for &T
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        T::fmt(self, f, db)
    }
}

impl<Db: ?Sized, T: ?Sized> DebugWithDb<Db> for Box<T>
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        T::fmt(self, f, db)
    }
}

impl<Db: ?Sized, T> DebugWithDb<Db> for Rc<T>
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        T::fmt(self, f, db)
    }
}

impl<Db: ?Sized, T: ?Sized> DebugWithDb<Db> for Arc<T>
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        T::fmt(self, f, db)
    }
}

impl<Db: ?Sized, T> DebugWithDb<Db> for Vec<T>
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        let elements = self.iter().map(|e| e.debug(db));
        f.debug_list().entries(elements).finish()
    }
}

impl<Db: ?Sized, T> DebugWithDb<Db> for Option<T>
where
    T: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        let me = self.as_ref().map(|v| v.debug(db));
        fmt::Debug::fmt(&me, f)
    }
}

impl<Db: ?Sized, K, V, S> DebugWithDb<Db> for HashMap<K, V, S>
where
    K: DebugWithDb<Db>,
    V: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        let elements = self.iter().map(|(k, v)| (k.debug(db), v.debug(db)));
        f.debug_map().entries(elements).finish()
    }
}

impl<Db: ?Sized, A, B> DebugWithDb<Db> for (A, B)
where
    A: DebugWithDb<Db>,
    B: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        f.debug_tuple("")
            .field(&self.0.debug(db))
            .field(&self.1.debug(db))
            .finish()
    }
}

impl<Db: ?Sized, A, B, C> DebugWithDb<Db> for (A, B, C)
where
    A: DebugWithDb<Db>,
    B: DebugWithDb<Db>,
    C: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        f.debug_tuple("")
            .field(&self.0.debug(db))
            .field(&self.1.debug(db))
            .field(&self.2.debug(db))
            .finish()
    }
}

impl<Db: ?Sized, V, S> DebugWithDb<Db> for HashSet<V, S>
where
    V: DebugWithDb<Db>,
    Db: AsSalsaDatabase,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>, db: &Db) -> fmt::Result {
        let elements = self.iter().map(|e| e.debug(db));
        f.debug_list().entries(elements).finish()
    }
}

/// This is used by the macro generated code.
/// If the field type implements `DebugWithDb`, uses that, otherwise, uses `Debug`.
/// That's the "has impl" trick (https://github.com/nvzqz/impls#how-it-works)
#[doc(hidden)]
pub mod helper {
    use super::{AsSalsaDatabase, DebugWith, DebugWithDb};
    use std::{fmt, marker::PhantomData};

    pub trait Fallback<T: fmt::Debug, Db: ?Sized> {
        fn salsa_debug<'a>(a: &'a T, _db: &Db) -> &'a dyn fmt::Debug {
            a
        }
    }

    impl<Everything, Db: ?Sized, T: fmt::Debug> Fallback<T, Db> for Everything {}

    pub struct SalsaDebug<T, Db: ?Sized>(PhantomData<T>, PhantomData<Db>);

    impl<T, Db: ?Sized> SalsaDebug<T, Db>
    where
        T: DebugWithDb<Db>,
        Db: AsSalsaDatabase,
    {
        #[allow(dead_code)]
        pub fn salsa_debug<'a>(a: &'a T, db: &'a Db) -> DebugWith<'a, Db> {
            a.debug(db)
        }
    }
}
