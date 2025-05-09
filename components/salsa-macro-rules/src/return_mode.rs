/// Generate the expression for the return type, depending on the return mode defined in [`salsa-macros::options::Options::returns`]
///
/// Used when generating field getters.
#[macro_export]
macro_rules! return_mode_expression {
    (
        (copy, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        *$field_ref_expr
    };

    (
        (clone, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        ::core::clone::Clone::clone($field_ref_expr)
    };

    (
        (ref, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        $field_ref_expr
    };

    (
        (deref, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        ::core::ops::Deref::deref($field_ref_expr)
    };

    (
        (as_ref, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        ::salsa::SalsaAsRef::as_ref($field_ref_expr)
    };

    (
        (as_deref, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        ::salsa::SalsaAsDeref::as_deref($field_ref_expr)
    };
}

#[macro_export]
macro_rules! return_mode_ty {
    (
        (copy, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        $field_ty
    };

    (
        (clone, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        $field_ty
    };

    (
        (ref, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        & $db_lt $field_ty
    };

    (
        (deref, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        & $db_lt <$field_ty as ::core::ops::Deref>::Target
    };

    (
        (as_ref, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        <$field_ty as ::salsa::SalsaAsRef>::AsRef<$db_lt>
    };

    (
        (as_deref, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        <$field_ty as ::salsa::SalsaAsDeref>::AsDeref<$db_lt>
    };
}
