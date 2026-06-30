#![allow(clippy::undocumented_unsafe_blocks)] // Implementations are structural; see trait safety docs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;

#[cfg(feature = "rayon")]
use rayon::iter::Either;

use crate::sync::Arc;

/// A value Salsa can safely retain across revisions.
///
/// Salsa values can be stored in interned and tracked structs or used as
/// tracked query results. Ordinary `'static` values are accepted directly at
/// those storage boundaries; types carrying the database lifetime must
/// implement this trait.
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
/// `#[derive(SalsaValue)]` checks this requirement structurally. It cannot
/// account for additional invariants maintained by unsafe code in the type's
/// methods.
///
/// For example, this derive is invalid even though its fields pass the
/// structural checks:
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
/// The author of that unsafe abstraction is responsible for not deriving
/// `SalsaValue`: Salsa may retain `InvalidValue` after the pointed-to memoized
/// result has been replaced.
#[diagnostic::on_unimplemented(
    message = "`{Self}` doesn't implement `SalsaValue`",
    note = "add `#[derive(salsa::SalsaValue)]` to `{Self}`"
)]
pub unsafe trait SalsaValue {}

/// Selects an explicit [`SalsaValue`] implementation or an unchanged `'static` value.
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

    pub const fn assert_salsa_value<T: SalsaValue>() {}
}

macro_rules! salsa_values {
    ($($ty:ty),* $(,)?) => {
        $(
            // SAFETY: Values of this type contain no database-lifetime references.
            unsafe impl SalsaValue for $ty {}
        )*
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
    crate::Id,
    Box<str>,
}

#[cfg(feature = "compact_str")]
salsa_values!(compact_str::CompactString);

// SAFETY: A genuinely `'static` reference remains valid across revisions.
unsafe impl<T: ?Sized> SalsaValue for &'static T {}

// SAFETY: `PhantomData` contains no data.
unsafe impl<T: ?Sized> SalsaValue for PhantomData<T> {}

// SAFETY: The parameter is `'static` and therefore contains no database-lifetime references.
unsafe impl<T: 'static> SalsaValue for std::hash::BuildHasherDefault<T> {}

macro_rules! container_salsa_value {
    ($($container:ident)::+ < $($parameter:ident),+ >; unchanged $state:ident) => {
        // SAFETY: Every retained parameter is a `SalsaValue`; the state is `'static`.
        unsafe impl<$($parameter),+, $state> SalsaValue
            for $($container)::+<$($parameter),+, $state>
        where
            $($parameter: SalsaValue),+,
            $state: 'static,
        {
        }
    };
    ($($container:ident)::+ < $($parameter:ident),+ >) => {
        // SAFETY: Every retained parameter is a `SalsaValue`.
        unsafe impl<$($parameter),+> SalsaValue for $($container)::+<$($parameter),+>
        where
            $($parameter: SalsaValue),+
        {
        }
    };
}

container_salsa_value!(Vec<T>);
container_salsa_value!(Option<T>);
container_salsa_value!(Result<T, E>);
container_salsa_value!(Box<T>);
container_salsa_value!(Arc<T>);
container_salsa_value!(thin_vec::ThinVec<T>);
container_salsa_value!(std::ops::Range<T>);
container_salsa_value!(std::ops::RangeInclusive<T>);

#[cfg(feature = "triomphe")]
container_salsa_value!(triomphe::Arc<T>);

// SAFETY: Every retained element is a `SalsaValue`.
unsafe impl<T: SalsaValue, const N: usize> SalsaValue for [T; N] {}
unsafe impl<T: SalsaValue> SalsaValue for Box<[T]> {}
unsafe impl<T: SalsaValue, const N: usize> SalsaValue for smallvec::SmallVec<[T; N]> where
    [T; N]: smallvec::Array<Item = T>
{
}

container_salsa_value!(HashMap<K, V>; unchanged S);
container_salsa_value!(HashSet<K>; unchanged S);
container_salsa_value!(BTreeMap<K, V>);
container_salsa_value!(BTreeSet<K>);
container_salsa_value!(hashbrown::HashMap<K, V>; unchanged S);
container_salsa_value!(hashbrown::HashSet<K>; unchanged S);
container_salsa_value!(indexmap::IndexMap<K, V>; unchanged S);
container_salsa_value!(indexmap::IndexSet<K>; unchanged S);

#[cfg(feature = "ordermap")]
container_salsa_value!(ordermap::OrderMap<K, V>; unchanged S);

#[cfg(feature = "ordermap")]
container_salsa_value!(ordermap::OrderSet<K>; unchanged S);

#[cfg(feature = "rayon")]
container_salsa_value!(Either<L, R>);

macro_rules! tuple_salsa_value {
    ($($t:ident),*) => {
        // SAFETY: Every retained tuple element is a `SalsaValue`.
        unsafe impl<$($t),*> SalsaValue for ($($t,)*)
        where
            $($t: SalsaValue),*
        {
        }
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
