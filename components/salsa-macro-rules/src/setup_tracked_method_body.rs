#[macro_export]
macro_rules! setup_method_body {
    (
        salsa_tracked_attr: #[$salsa_tracked_attr:meta],
        self: $self:ident,
        self_ty: $self_ty:ty,
        db_lt: $($db_lt:lifetime)?,
        db: $db:ident,
        db_ty: ($($db_ty:tt)*),
        input_ids: [$($input_id:ident),*],
        input_tys: [$($input_ty:ty),*],
        output_ty: $output_ty:ty,
        inner_fn_name: $inner_fn_name:ident,
        inner_fn: $inner_fn:item,

        // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
        // We have the procedural macro generate names for those items that are
        // not used elsewhere in the user's code.
        unused_names: [
            $InnerTrait:ident,
        ]
    ) => {
        {
            trait $InnerTrait<$($db_lt)?> {
                fn $inner_fn_name($self, db: $($db_ty)*, $($input_id: $input_ty),*) -> $output_ty;
            }

            impl<$($db_lt)?> $InnerTrait<$($db_lt)?> for $self_ty {
                $inner_fn
            }

            #[$salsa_tracked_attr]
            fn $inner_fn_name<$($db_lt)?>(db: $($db_ty)*, this: $self_ty, $($input_id: $input_ty),*) -> $output_ty {
                <$self_ty as $InnerTrait>::$inner_fn_name(this, db, $($input_id),*)
            }

            $inner_fn_name($db, $self, $($input_id),*)
        }
    };
}
