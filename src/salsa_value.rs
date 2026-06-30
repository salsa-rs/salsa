#![allow(clippy::undocumented_unsafe_blocks)] // Implementations are structural; see trait safety docs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;

#[cfg(feature = "rayon")]
use rayon::iter::Either;

use crate::sync::Arc;

/// A value that Salsa can safely retain across revisions.
///
/// Salsa values can be stored in input, interned, and tracked structs, or
/// used as tracked query results.
///
/// Salsa's macros accept ordinary `'static` values automatically when checking
/// a complete stored value, such as input storage or a query result. Fields in
/// interned and tracked structs can carry the database lifetime and must
/// implement this trait. Implement or derive it only for types that are safe to
/// retain across revisions, such as Salsa handles.
///
/// # Safety
///
/// Implementing this trait asserts that the type is effectively `'static`
/// from Salsa's perspective: an instance produced in an older revision must
/// remain safe to retain and use in a newer revision. This includes calling
/// safe trait methods such as [`PartialEq::eq`] on the old value. In particular,
/// `SalsaValue` must not be implemented for a database-lifetime reference such
/// as `&'db T`, which may point to storage changed or freed in a newer revision.
///
/// `#[derive(SalsaValue)]` checks this requirement structurally by requiring
/// every field to implement `SalsaValue`. These checks cannot account for
/// additional invariants maintained by unsafe code in the type's methods.
///
/// For example, this derive is invalid even though both fields implement
/// `SalsaValue`:
///
/// ```no_run
/// #[salsa::interned]
/// struct Interned<'db> {
///     value: u32,
/// }
///
/// #[derive(salsa::SalsaValue)]
/// struct InvalidValue<'db> {
///     // This address points into another query's memoized result.
///     address: usize,
///     witness: Interned<'db>,
/// }
///
/// impl<'db> InvalidValue<'db> {
///     fn value(&self) -> &'db u32 {
///         let _ = self.witness;
///         unsafe { &*(self.address as *const u32) }
///     }
/// }
/// ```
///
/// An unsafe abstraction can use the `Interned<'db>` field as a lifetime
/// witness that prevents the pointer from outliving the database borrow in
/// ordinary Rust. Salsa may, however, retain `InvalidValue` after the query
/// containing the pointed-to value re-executes. The author of the unsafe code
/// is therefore responsible for not deriving `SalsaValue` when an invariant
/// like this would be invalidated across revisions.
///
/// A field annotated with
/// `#[salsa_value(prove_safe_to_retain_manually)]` is exempt from the structural
/// check. The annotation asserts that retaining that field is safe under this
/// contract.
pub unsafe trait SalsaValue {}

/// Selects an explicit [`SalsaValue`] impl or the automatic `'static` fallback
/// for a complete stored value.
#[doc(hidden)]
pub mod helper {
    use std::marker::PhantomData;

    use super::SalsaValue;

    pub struct Dispatch<T>(PhantomData<T>);

    impl<T: SalsaValue> Dispatch<T> {
        pub fn assert_salsa_value() {}
    }

    pub trait Fallback<T> {
        fn assert_salsa_value();
    }

    impl<T: 'static> Fallback<T> for Dispatch<T> {
        fn assert_salsa_value() {}
    }
}

macro_rules! salsa_values {
    ($($ty:ty),* $(,)?) => {
        $(unsafe impl SalsaValue for $ty {})*
    };
}

salsa_values! {
    (),
    bool,
    char,
    f32,
    f64,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    String,
    PathBuf,
    std::collections::hash_map::RandomState,
    std::convert::Infallible,
    std::num::NonZeroI8,
    std::num::NonZeroI16,
    std::num::NonZeroI32,
    std::num::NonZeroI64,
    std::num::NonZeroI128,
    std::num::NonZeroIsize,
    std::num::NonZeroU8,
    std::num::NonZeroU16,
    std::num::NonZeroU32,
    std::num::NonZeroU64,
    std::num::NonZeroU128,
    std::num::NonZeroUsize,
}

#[cfg(feature = "compact_str")]
salsa_values!(compact_str::CompactString);

unsafe impl<T: ?Sized> SalsaValue for &'static T {}
unsafe impl<T: ?Sized> SalsaValue for PhantomData<T> {}
unsafe impl<T: 'static> SalsaValue for std::hash::BuildHasherDefault<T> {}
unsafe impl<T: SalsaValue> SalsaValue for std::ops::Range<T> {}
unsafe impl<T: SalsaValue> SalsaValue for std::ops::RangeInclusive<T> {}
unsafe impl SalsaValue for crate::Id {}
unsafe impl<T: SalsaValue> SalsaValue for Vec<T> {}
unsafe impl<T: SalsaValue> SalsaValue for thin_vec::ThinVec<T> {}
unsafe impl<A> SalsaValue for smallvec::SmallVec<A>
where
    A: smallvec::Array,
    A::Item: SalsaValue,
{
}
unsafe impl<T: SalsaValue> SalsaValue for Option<T> {}
unsafe impl<T: SalsaValue, E: SalsaValue> SalsaValue for Result<T, E> {}
unsafe impl<T: SalsaValue, const N: usize> SalsaValue for [T; N] {}
unsafe impl<T: SalsaValue> SalsaValue for Box<T> {}
unsafe impl<T: SalsaValue> SalsaValue for Box<[T]> {}
unsafe impl SalsaValue for Box<str> {}
unsafe impl<T: SalsaValue> SalsaValue for Arc<T> {}

#[cfg(feature = "triomphe")]
unsafe impl<T: SalsaValue> SalsaValue for triomphe::Arc<T> {}

unsafe impl<K, S> SalsaValue for HashSet<K, S>
where
    K: SalsaValue,
    S: 'static,
{
}

unsafe impl<K, V, S> SalsaValue for HashMap<K, V, S>
where
    K: SalsaValue,
    V: SalsaValue,
    S: 'static,
{
}

unsafe impl<K: SalsaValue> SalsaValue for BTreeSet<K> {}
unsafe impl<K: SalsaValue, V: SalsaValue> SalsaValue for BTreeMap<K, V> {}

unsafe impl<K, S> SalsaValue for hashbrown::HashSet<K, S>
where
    K: SalsaValue,
    S: 'static,
{
}

unsafe impl<K, V, S> SalsaValue for hashbrown::HashMap<K, V, S>
where
    K: SalsaValue,
    V: SalsaValue,
    S: 'static,
{
}

unsafe impl<K, S> SalsaValue for indexmap::IndexSet<K, S>
where
    K: SalsaValue,
    S: 'static,
{
}

unsafe impl<K, V, S> SalsaValue for indexmap::IndexMap<K, V, S>
where
    K: SalsaValue,
    V: SalsaValue,
    S: 'static,
{
}

#[cfg(feature = "ordermap")]
unsafe impl<K, S> SalsaValue for ordermap::OrderSet<K, S>
where
    K: SalsaValue,
    S: 'static,
{
}

#[cfg(feature = "ordermap")]
unsafe impl<K, V, S> SalsaValue for ordermap::OrderMap<K, V, S>
where
    K: SalsaValue,
    V: SalsaValue,
    S: 'static,
{
}

#[cfg(feature = "rayon")]
unsafe impl<L: SalsaValue, R: SalsaValue> SalsaValue for Either<L, R> {}

macro_rules! tuple_salsa_value {
    ($($t:ident),*) => {
        unsafe impl<$($t),*> SalsaValue for ($($t,)*)
        where
            $($t: SalsaValue,)*
        {}
    };
}

tuple_salsa_value!(A);
tuple_salsa_value!(A, B);
tuple_salsa_value!(A, B, C);
tuple_salsa_value!(A, B, C, D);
tuple_salsa_value!(A, B, C, D, E);
tuple_salsa_value!(A, B, C, D, E, F);
tuple_salsa_value!(A, B, C, D, E, F, G);
tuple_salsa_value!(A, B, C, D, E, F, G, H);
tuple_salsa_value!(A, B, C, D, E, F, G, H, I);
tuple_salsa_value!(A, B, C, D, E, F, G, H, I, J);
tuple_salsa_value!(A, B, C, D, E, F, G, H, I, J, K);
tuple_salsa_value!(A, B, C, D, E, F, G, H, I, J, K, L);
