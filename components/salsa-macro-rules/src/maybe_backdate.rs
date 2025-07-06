/// Conditionally update field value and backdate revisions
#[macro_export]
macro_rules! maybe_backdate {
    (
        ($return_mode:ident, no_backdate, $maybe_default:ident),
        $maybe_update:tt,
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
        ($return_mode:ident, backdate, $maybe_default:ident),
        $maybe_update:tt,
        $old_field_place:expr,
        $new_field_place:expr,
        $revision_place:expr,
        $current_revision:expr,
        $zalsa:ident,
     ) => {
        if $maybe_update(std::ptr::addr_of_mut!($old_field_place), $new_field_place) {
            $revision_place.store($current_revision);
        }
    };
}

/// Conditionally update field value and backdate revisions
#[macro_export]
macro_rules! maybe_backdate_late {
    (
        ($return_mode:ident, no_backdate, $maybe_default:ident),
        $maybe_update:tt,
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
        ($return_mode:ident, backdate, $maybe_default:ident),
        $maybe_update:tt,
        $old_field_place:expr,
        $new_field_place:expr,
        $revision_place:expr,
        $current_revision:expr,
        $zalsa:ident,
     ) => {
        if $zalsa::LateField::maybe_update(&mut $old_field_place, $new_field_place, $maybe_update, $revision_place.load()) {
            $revision_place.store($current_revision);
        }
    };
}