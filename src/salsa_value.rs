#![allow(clippy::undocumented_unsafe_blocks)] // Implementations are structural; see trait safety docs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, align_of, size_of};
use std::path::PathBuf;

#[cfg(feature = "rayon")]
use rayon::iter::Either;

use crate::sync::Arc;

/// A value Salsa can retain and later expose with the database lifetime `'db`.
///
/// `Self` is the `'static` representation retained in Salsa's storage and
/// [`WithDb`](SalsaValue::WithDb) is the same value with the database lifetime
/// restored.
///
/// # Safety
///
/// `WithDb` is not an alternative storage representation. `Self` and `WithDb`
/// must represent the same value modulo database lifetime branding, with
/// identical layouts and validity invariants.
///
/// Reinterpreting a shared `Self` reference as `WithDb` must be sound for
/// `'db`, and a `WithDb` value must remain valid when Salsa retains it as
/// `Self`. This includes calling safe trait methods such as [`PartialEq::eq`]
/// in a later revision.
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
pub unsafe trait SalsaValue<'db>: Sized + 'static + Send + Sync {
    /// The same value as `Self` with the database lifetime restored.
    type WithDb: 'db + Send + Sync;
}

/// Selects an explicit [`SalsaValue`] implementation or an unchanged `'static` value.
#[doc(hidden)]
pub mod helper {
    use std::marker::PhantomData;

    use super::SalsaValue;

    pub struct Dispatch<'db, T, WithDb>(
        PhantomData<&'db ()>,
        PhantomData<fn() -> T>,
        PhantomData<fn() -> WithDb>,
    );

    impl<T, WithDb> Dispatch<'_, T, WithDb> {
        pub const VALUE: Self = Self(PhantomData, PhantomData, PhantomData);
    }

    pub trait Fallback {
        fn assert_salsa_value(self);
    }

    impl<'db, T, WithDb> Fallback for &&Dispatch<'db, T, WithDb>
    where
        T: SalsaValue<'db, WithDb = WithDb>,
    {
        fn assert_salsa_value(self) {}
    }

    impl<T: 'static + Send + Sync> Fallback for &Dispatch<'_, T, T> {
        fn assert_salsa_value(self) {}
    }

    #[diagnostic::on_unimplemented(
        message = "the field type `{Self}` does not implement `SalsaValue`",
        label = "does not implement `SalsaValue`",
        note = "derive `salsa::SalsaValue` for local field types; for foreign types, use `#[salsa_value(prove_safe_to_retain_manually)]` only after verifying retention is safe"
    )]
    pub trait SalsaValueField<'db, Static> {}

    #[diagnostic::do_not_recommend]
    impl<'db, Static, WithDb> SalsaValueField<'db, Static> for WithDb where
        Static: SalsaValue<'db, WithDb = WithDb>
    {
    }

    pub const fn assert_salsa_value<'db, Static, Output>(_: PhantomData<&'db ()>)
    where
        Output: SalsaValueField<'db, Static>,
    {
    }

    #[diagnostic::on_unimplemented(
        message = "the tracked function's return type `{Self}` does not implement `SalsaValue`",
        label = "does not implement `SalsaValue`",
        note = "consider deriving `salsa::SalsaValue` for the tracked function's return type if it is safe to retain across revisions"
    )]
    pub trait SalsaValueOutput<'db, Static> {}

    #[diagnostic::do_not_recommend]
    impl<'db, Static, Output> SalsaValueOutput<'db, Static> for Output where
        Static: SalsaValue<'db, WithDb = Output>
    {
    }

    pub const fn assert_salsa_value_output<'db, Static, Output>(_: PhantomData<&'db ()>)
    where
        Output: SalsaValueOutput<'db, Static>,
    {
    }
}

macro_rules! identity_salsa_values {
    ($($ty:ty),* $(,)?) => {
        $(
            // SAFETY: The representation is unchanged for every `'db`.
            unsafe impl<'db> SalsaValue<'db> for $ty {
                type WithDb = Self;
            }
        )*
    };
}

identity_salsa_values! {
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
}

#[cfg(feature = "compact_str")]
identity_salsa_values!(compact_str::CompactString);

// SAFETY: A genuinely `'static` reference is unchanged by rebinding.
unsafe impl<T: ?Sized + Sync + 'static> SalsaValue<'_> for &'static T {
    type WithDb = Self;
}

// SAFETY: `PhantomData` contains no data. These implementations preserve the
// lifetime branding used by generated and user-defined Salsa values.
unsafe impl<'db, T: ?Sized + Sync + 'static> SalsaValue<'db> for PhantomData<&'static T> {
    type WithDb = PhantomData<&'db T>;
}

unsafe impl<'db, T: ?Sized + Sync + 'static> SalsaValue<'db> for PhantomData<fn() -> &'static T> {
    type WithDb = PhantomData<fn() -> &'db T>;
}

// SAFETY: The representation is unchanged for every `'db`.
unsafe impl<T: 'static + Send + Sync> SalsaValue<'_> for std::hash::BuildHasherDefault<T> {
    type WithDb = Self;
}

macro_rules! container_salsa_value {
    ($($container:ident)::+ < $($parameter:ident),+ >; unchanged $state:ident) => {
        // SAFETY: The container preserves its layout when its parameters are rebound.
        unsafe impl<'db, $($parameter),+, $state> SalsaValue<'db>
            for $($container)::+<$($parameter),+, $state>
        where
            $($parameter: SalsaValue<'db>),+,
            $state: 'static + Send + Sync,
        {
            type WithDb = $($container)::+<
                $(<$parameter as SalsaValue<'db>>::WithDb),+,
                $state,
            >;
        }
    };
    ($($container:ident)::+ < $($parameter:ident),+ >) => {
        // SAFETY: The container preserves its layout when its parameters are rebound.
        unsafe impl<'db, $($parameter),+> SalsaValue<'db>
            for $($container)::+<$($parameter),+>
        where
            $($parameter: SalsaValue<'db>),+
        {
            type WithDb = $($container)::+<$(<$parameter as SalsaValue<'db>>::WithDb),+>;
        }
    };
}

container_salsa_value!(Vec<T>);
container_salsa_value!(Option<T>);
container_salsa_value!(Result<T, E>);
container_salsa_value!(Box<T>);
container_salsa_value!(Arc<T>);
container_salsa_value!(thin_vec::ThinVec<T>);

#[cfg(feature = "triomphe")]
container_salsa_value!(triomphe::Arc<T>);

unsafe impl<'db, T, const N: usize> SalsaValue<'db> for [T; N]
where
    T: SalsaValue<'db>,
{
    type WithDb = [<T as SalsaValue<'db>>::WithDb; N];
}

unsafe impl<'db, T> SalsaValue<'db> for Box<[T]>
where
    T: SalsaValue<'db>,
{
    type WithDb = Box<[<T as SalsaValue<'db>>::WithDb]>;
}

unsafe impl<'db, T, const N: usize> SalsaValue<'db> for smallvec::SmallVec<[T; N]>
where
    T: SalsaValue<'db>,
    [T; N]: smallvec::Array<Item = T>,
    [<T as SalsaValue<'db>>::WithDb; N]: smallvec::Array<Item = <T as SalsaValue<'db>>::WithDb>,
{
    type WithDb = smallvec::SmallVec<[<T as SalsaValue<'db>>::WithDb; N]>;
}

identity_salsa_values!(Box<str>);

container_salsa_value!(std::ops::Range<T>);
container_salsa_value!(std::ops::RangeInclusive<T>);

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
        // SAFETY: Tuples preserve their layout when their elements are rebound.
        unsafe impl<'db, $($t),*> SalsaValue<'db> for ($($t,)*)
        where
            $($t: SalsaValue<'db>),*
        {
            type WithDb = ($(<$t as SalsaValue<'db>>::WithDb,)*);
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

/// Erases the database lifetime before retaining a value in Salsa storage.
///
/// # Safety
///
/// The returned value must only be rebound while used with the database from
/// which `value` originated.
pub(crate) unsafe fn erase<'db, F>(value: <F as SalsaValue<'db>>::WithDb) -> F
where
    F: SalsaValue<'db>,
{
    const {
        assert!(size_of::<F>() == size_of::<<F as SalsaValue<'db>>::WithDb>());
        assert!(align_of::<F>() == align_of::<<F as SalsaValue<'db>>::WithDb>());
    }

    let value = ManuallyDrop::new(value);
    // SAFETY: Guaranteed by `F`'s `SalsaValue` implementation.
    unsafe { std::mem::transmute_copy(&value) }
}

/// Restores the database lifetime for a retained value.
pub(crate) fn rebind<'db, F>(value: &'db F) -> &'db <F as SalsaValue<'db>>::WithDb
where
    F: SalsaValue<'db>,
{
    const {
        assert!(size_of::<F>() == size_of::<<F as SalsaValue<'db>>::WithDb>());
        assert!(align_of::<F>() == align_of::<<F as SalsaValue<'db>>::WithDb>());
    }

    // SAFETY: Guaranteed by `F`'s `SalsaValue` implementation. The restored
    // lifetime cannot outlive the shared borrow of the retained value.
    unsafe { std::mem::transmute(value) }
}

/// Restores the database lifetime for an exclusively borrowed retained value.
pub(crate) fn rebind_mut<'db, F>(value: &'db mut F) -> &'db mut <F as SalsaValue<'db>>::WithDb
where
    F: SalsaValue<'db>,
{
    const {
        assert!(size_of::<F>() == size_of::<<F as SalsaValue<'db>>::WithDb>());
        assert!(align_of::<F>() == align_of::<<F as SalsaValue<'db>>::WithDb>());
    }

    // SAFETY: Guaranteed by `F`'s `SalsaValue` implementation. The restored
    // lifetime cannot outlive the exclusive borrow of the retained value.
    unsafe { std::mem::transmute(value) }
}
