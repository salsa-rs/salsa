/// Conditionally update field value and backdate revisions
#[macro_export]
macro_rules! maybe_backdate {
    (
        ($maybe_clone:ident, no_backdate, $maybe_default:ident),
        $field_ty:ty,
        $old_field_place:expr,
        $new_field_place:expr,
        $revision_place:expr,
        $current_revision:expr,
        $zalsa:ident,

    ) => {
        $zalsa::always_update(
            &mut $revision_place,
            $current_revision,
            &mut $old_field_place,
            $new_field_place,
        );
    };

    (
        ($maybe_clone:ident, backdate, $maybe_default:ident),
        $field_ty:ty,
        $old_field_place:expr,
        $new_field_place:expr,
        $revision_place:expr,
        $current_revision:expr,
        $zalsa:ident,
     ) => {
        if $zalsa::UpdateDispatch::<$field_ty>::maybe_update(
            std::ptr::addr_of_mut!($old_field_place),
            $new_field_place,
        ) {
            $revision_place = $current_revision;
        }
    };
}
