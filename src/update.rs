use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    hash::{BuildHasher, Hash},
    path::PathBuf,
};

use crate::Revision;

/// This is used by the macro generated code.
/// If possible, uses `Update` trait, but else requires `'static`.
///
/// To use:
///
/// ```rust,ignore
/// use crate::update::helper::Fallback;
/// update::helper::Dispatch::<$ty>::maybe_update(pointer, new_value);
/// ```
///
/// It is important that you specify the `$ty` explicitly.
///
/// This uses the ["method dispatch hack"](https://github.com/nvzqz/impls#how-it-works)
/// to use the `Update` trait if it is available and else fallback to `'static`.
pub mod helper {
    use std::marker::PhantomData;

    use super::{update_fallback, Update};

    pub struct Dispatch<D>(PhantomData<D>);

    impl<D> Default for Dispatch<D> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<D> Dispatch<D> {
        pub fn new() -> Self {
            Dispatch(PhantomData)
        }
    }

    impl<D> Dispatch<D>
    where
        D: Update,
    {
        /// # Safety
        ///
        /// See the `maybe_update` method in the [`Update`][] trait.
        pub unsafe fn maybe_update(old_pointer: *mut D, new_value: D) -> bool {
            unsafe { D::maybe_update(old_pointer, new_value) }
        }
    }

    /// # Safety
    ///
    /// Impl will fulfill the postconditions of `maybe_update`
    pub unsafe trait Fallback<T> {
        /// # Safety
        ///
        /// Same safety conditions as `Update::maybe_update`
        unsafe fn maybe_update(old_pointer: *mut T, new_value: T) -> bool;
    }

    unsafe impl<T: 'static + PartialEq> Fallback<T> for Dispatch<T> {
        unsafe fn maybe_update(old_pointer: *mut T, new_value: T) -> bool {
            unsafe { update_fallback(old_pointer, new_value) }
        }
    }
}

/// "Fallback" for maybe-update that is suitable for fully owned T
/// that implement `Eq`. In this version, we update only if the new value
/// is not `Eq` to the old one. Note that given `Eq` impls that are not just
/// structurally comparing fields, this may cause us not to update even if
/// the value has changed (presumably because this change is not semantically
/// significant).
///
/// # Safety
///
/// See `Update::maybe_update`
pub unsafe fn update_fallback<T>(old_pointer: *mut T, new_value: T) -> bool
where
    T: 'static + PartialEq,
{
    // Because everything is owned, this ref is simply a valid `&mut`
    let old_ref: &mut T = unsafe { &mut *old_pointer };

    if *old_ref != new_value {
        *old_ref = new_value;
        true
    } else {
        // Subtle but important: Eq impls can be buggy or define equality
        // in surprising ways. If it says that the value has not changed,
        // we do not modify the existing value, and thus do not have to
        // update the revision, as downstream code will not see the new value.
        false
    }
}

/// Helper for generated code. Updates `*old_pointer` with `new_value`
/// and updates `*old_revision` with `new_revision.` Used for fields
/// tagged with `#[no_eq]`
pub fn always_update<T>(
    old_revision: &mut Revision,
    new_revision: Revision,
    old_pointer: &mut T,
    new_value: T,
) {
    *old_revision = new_revision;
    *old_pointer = new_value;
}

/// # Safety
///
/// Implementing this trait requires the implementor to verify:
///
/// * `maybe_update` ensures the properties it is intended to ensure.
/// * If the value implements `Eq`, it is safe to compare an instance
///   of the value from an older revision with one from the newer
///   revision. If the value compares as equal, no update is needed to
///   bring it into the newer revision.
///
/// NB: The second point implies that `Update` cannot be implemented for any
/// `&'db T` -- (i.e., any Rust reference tied to the database).
/// Such a value could refer to memory that was freed in some
/// earlier revision. Even if the memory is still valid, it could also
/// have been part of a tracked struct whose values were mutated,
/// thus invalidating the `'db` lifetime (from a stacked borrows perspective).
/// Either way, the `Eq` implementation would be invalid.
pub unsafe trait Update {
    /// # Returns
    ///
    /// True if the value should be considered to have changed in the new revision.
    ///
    /// # Safety
    ///
    /// ## Requires
    ///
    /// Informally, requires that `old_value` points to a value in the
    /// database that is potentially from a previous revision and `new_value`
    /// points to a value produced in this revision.
    ///
    /// More formally, requires that
    ///
    /// * all parameters meet the [validity and safety invariants][i] for their type
    /// * `old_value` further points to allocated memory that meets the [validity invariant][i] for `Self`
    /// * all data *owned* by `old_value` further meets its safety invariant
    ///     * not that borrowed data in `old_value` only meets its validity invariant
    ///       and hence cannot be dereferenced; essentially, a `&T` may point to memory
    ///       in the database which has been modified or even freed in the newer revision.
    ///
    /// [i]: https://www.ralfj.de/blog/2018/08/22/two-kinds-of-invariants.html
    ///
    /// ## Ensures
    ///
    /// That `old_value` is updated with
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool;
}

unsafe impl<T> Update for Vec<T>
where
    T: Update,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_vec: Self) -> bool {
        let old_vec: &mut Vec<T> = unsafe { &mut *old_pointer };

        if old_vec.len() != new_vec.len() {
            old_vec.clear();
            old_vec.extend(new_vec);
            return true;
        }

        let mut changed = false;
        for (old_element, new_element) in old_vec.iter_mut().zip(new_vec) {
            changed |= T::maybe_update(old_element, new_element);
        }

        changed
    }
}

macro_rules! maybe_update_set {
    ($old_pointer: expr, $new_set: expr) => {{
        let old_pointer = $old_pointer;
        let new_set = $new_set;

        let old_set: &mut Self = unsafe { &mut *old_pointer };

        if *old_set == new_set {
            false
        } else {
            old_set.clear();
            old_set.extend(new_set);
            return true;
        }
    }};
}

unsafe impl<K, S> Update for HashSet<K, S>
where
    K: Update + Eq + Hash,
    S: BuildHasher,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_set: Self) -> bool {
        maybe_update_set!(old_pointer, new_set)
    }
}

unsafe impl<K> Update for BTreeSet<K>
where
    K: Update + Eq + Ord,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_set: Self) -> bool {
        maybe_update_set!(old_pointer, new_set)
    }
}

// Duck typing FTW, it was too annoying to make a proper function out of this.
macro_rules! maybe_update_map {
    ($old_pointer: expr, $new_map: expr) => {
        'function: {
            let old_pointer = $old_pointer;
            let new_map = $new_map;
            let old_map: &mut Self = unsafe { &mut *old_pointer };

            // To be considered "equal", the set of keys
            // must be the same between the two maps.
            let same_keys =
                old_map.len() == new_map.len() && old_map.keys().all(|k| new_map.contains_key(k));

            // If the set of keys has changed, then just pull in the new values
            // from new_map and discard the old ones.
            if !same_keys {
                old_map.clear();
                old_map.extend(new_map);
                break 'function true;
            }

            // Otherwise, recursively descend to the values.
            // We do not invoke `K::update` because we assume
            // that if the values are `Eq` they must not need
            // updating (see the trait criteria).
            let mut changed = false;
            for (key, new_value) in new_map.into_iter() {
                let old_value = old_map.get_mut(&key).unwrap();
                changed |= V::maybe_update(old_value, new_value);
            }
            changed
        }
    };
}

unsafe impl<K, V, S> Update for HashMap<K, V, S>
where
    K: Update + Eq + Hash,
    V: Update,
    S: BuildHasher,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_map: Self) -> bool {
        maybe_update_map!(old_pointer, new_map)
    }
}

unsafe impl<K, V> Update for BTreeMap<K, V>
where
    K: Update + Eq + Ord,
    V: Update,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_map: Self) -> bool {
        maybe_update_map!(old_pointer, new_map)
    }
}

unsafe impl<T> Update for Box<T>
where
    T: Update,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_box: Self) -> bool {
        let old_box: &mut Box<T> = unsafe { &mut *old_pointer };

        T::maybe_update(&mut **old_box, *new_box)
    }
}

unsafe impl<T, const N: usize> Update for [T; N]
where
    T: Update,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_vec: Self) -> bool {
        let old_pointer: *mut T = std::ptr::addr_of_mut!((*old_pointer)[0]);
        let mut changed = false;
        for (new_element, i) in new_vec.into_iter().zip(0..) {
            changed |= T::maybe_update(old_pointer.add(i), new_element);
        }
        changed
    }
}

macro_rules! fallback_impl {
    ($($t:ty,)*) => {
        $(
            unsafe impl Update for $t {
                unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
                    update_fallback(old_pointer, new_value)
                }
            }
        )*
    }
}

fallback_impl! {
    String,
    i64,
    u64,
    i32,
    u32,
    i16,
    u16,
    i8,
    u8,
    bool,
    f32,
    f64,
    usize,
    isize,
    PathBuf,
}

macro_rules! tuple_impl {
    ($($t:ident),*; $($u:ident),*) => {
        unsafe impl<$($t),*> Update for ($($t,)*)
        where
            $($t: Update,)*
        {
            #[allow(non_snake_case)]
            unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
                let ($($t,)*) = new_value;
                let ($($u,)*) = unsafe { &mut *old_pointer };

                #[allow(unused_mut)]
                let mut changed = false;
                $(
                    unsafe { changed |= Update::maybe_update($u, $t); }
                )*
                changed
            }
        }
    }
}

// Create implementations for tuples up to arity 12
tuple_impl!(;);
tuple_impl!(A; a);
tuple_impl!(A, B; a, b);
tuple_impl!(A, B, C; a, b, c);
tuple_impl!(A, B, C, D; a, b, c, d);
tuple_impl!(A, B, C, D, E; a, b, c, d, e);
tuple_impl!(A, B, C, D, E, F; a, b, c, d, e, f);
tuple_impl!(A, B, C, D, E, F, G; a, b, c, d, e, f, g);
tuple_impl!(A, B, C, D, E, F, G, H; a, b, c, d, e, f, g, h);
tuple_impl!(A, B, C, D, E, F, G, H, I; a, b, c, d, e, f, g, h, i);
tuple_impl!(A, B, C, D, E, F, G, H, I, J; a, b, c, d, e, f, g, h, i, j);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K; a, b, c, d, e, f, g, h, i, j, k);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L; a, b, c, d, e, f, g, h, i, j, k, l);

unsafe impl<T> Update for Option<T>
where
    T: Update,
{
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_value = unsafe { &mut *old_pointer };
        match (old_value, new_value) {
            (Some(old), Some(new)) => T::maybe_update(old, new),
            (None, None) => false,
            (old_value, new_value) => {
                *old_value = new_value;
                true
            }
        }
    }
}
