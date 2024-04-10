use std::path::PathBuf;

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

    impl<D> Dispatch<D> {
        pub fn new() -> Self {
            Dispatch(PhantomData)
        }
    }

    impl<D> Dispatch<D>
    where
        D: Update,
    {
        pub unsafe fn maybe_update(old_pointer: *mut D, new_value: D) -> bool {
            unsafe { D::maybe_update(old_pointer, new_value) }
        }
    }

    pub unsafe trait Fallback<T> {
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
) where
    T: 'static,
{
    *old_revision = new_revision;
    *old_pointer = new_value;
}

/// The `unsafe` on the trait is to assert that `maybe_update` ensures
/// the properties it is intended to ensure.
pub unsafe trait Update {
    /// # Returns
    ///
    /// True if the value should be considered to have changed in the new revision.
    ///
    /// # Unsafe contract
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

unsafe impl<T> Update for &T {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_value: *const T = unsafe { *old_pointer };
        if old_value != (new_value as *const T) {
            unsafe {
                *old_pointer = new_value;
            }
            true
        } else {
            false
        }
    }
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
