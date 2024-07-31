/// Generate either `field_ref_expr` or `field_ty::default`
///
/// Used when generating an input's builder.
#[macro_export]
macro_rules! maybe_default {
    (
        ($maybe_clone:ident, $maybe_backdate:ident, default),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        <$field_ty>::default()
    };

    (
        ($maybe_clone:ident, $maybe_backdate:ident, required),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        $field_ref_expr
    };
}

#[macro_export]
macro_rules! maybe_default_tt {
    (($maybe_clone:ident, $maybe_backdate:ident, default) => $($t:tt)*) => {
        $($t)*
    };

    (($maybe_clone:ident, $maybe_backdate:ident, required) => $($t:tt)*) => {

    };
}
