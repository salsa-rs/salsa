/// Generate either `field_ref_expr` or a clone of that expr.
///
/// Used when generating field getters.
#[macro_export]
macro_rules! maybe_clone {
    (
        (no_clone, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        $field_ref_expr
    };

    (
        (clone, $maybe_backdate:ident, $maybe_default:ident),
        $field_ty:ty,
        $field_ref_expr:expr,
    ) => {
        std::clone::Clone::clone($field_ref_expr)
    };
}

#[macro_export]
macro_rules! maybe_cloned_ty {
    (
        (no_clone, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        & $db_lt $field_ty
    };

    (
        (clone, $maybe_backdate:ident, $maybe_default:ident),
        $db_lt:lifetime,
        $field_ty:ty
    ) => {
        $field_ty
    };
}
