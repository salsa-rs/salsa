// Returns `u` but with the lifetime of `t`.
//
// Safe if you know that data at `u` will remain shared
// until the reference `t` expires.
#[allow(clippy::needless_lifetimes)]
pub(crate) unsafe fn transmute_lifetime<'t, 'u, T, U>(_t: &'t T, u: &'u U) -> &'t U {
    std::mem::transmute(u)
}
