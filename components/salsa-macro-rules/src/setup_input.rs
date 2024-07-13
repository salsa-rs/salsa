/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_input {
    (
        // Visibility of the struct
        vis: $vis:vis,

        // Name of the struct
        Struct: $Struct:ident,

        // Name of the `'db` lifetime that the user gave
        db_lt: $db_lt:lifetime,

        // Name user gave for `new`
        new_fn: $new_fn:ident,

        // Field names
        field_ids: [$($field_id:ident),*],

        // Field types, may reference `db_lt`
        field_tys: [$($field_ty:ty),*],

        // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
        field_indices: [$($field_index:tt),*],

        // Indices of fields to be used for id computations
        id_field_indices: [$($id_field_index:tt),*],

        // Number of fields
        num_fields: $N:literal,

        // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
        // We have the procedural macro generate names for those items that are
        // not used elsewhere in the user's code.
        unused_names: [
            $zalsa:ident,
            $zalsa_struct:ident,
            $Configuration:ident,
            $CACHE:ident,
            $Db:ident,
            $NonNull:ident,
            $Revision:ident,
            $ValueStruct:ident,
        ]
    ) => {
        $vis struct $Struct<$db_lt> {

        }

        const _: () = {
            use salsa::plumbing as $zalsa;
            use $zalsa::input as $zalsa_struct;
            use $zalsa::Revision as $Revision;
            use std::ptr::NonNull as $NonNull;

            struct $Configuration;

            impl $zalsa::tracked_struct::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($Struct);

                const FIELD_DEBUG_NAMES: &'static [&'static str] = &[
                    $(stringify!($field_id),)*
                ];

                type Fields<$db_lt> = ($($field_ty,)*);

                type Revisions = [$Revision; $N];

                type Struct<$db_lt> = $Struct<$db_lt>;

                unsafe fn struct_from_raw<'db>(ptr: $NonNull<$ValueStruct<Self>>) -> Self::Struct<'db> {
                    $Struct(ptr, std::marker::PhantomData)
                }

                fn deref_struct(s: Self::Struct<'_>) -> &$ValueStruct<Self> {
                    unsafe { s.0.as_ref() }
                }

                fn id_fields(fields: &Self::Fields<'_>) -> impl Hash {
                    ( $( &fields.$id_field_index ),* )
                }

                fn revision(revisions: &Self::Revisions, field_index: u32) -> $Revision {
                    revisions[field_index as usize]
                }

                fn new_revisions(current_revision: $Revision) -> Self::Revisions {
                    [current_revision; $N]
                }

                unsafe fn update_fields<'db>(
                    current_revision: Revision,
                    revisions: &mut Self::Revisions,
                    old_fields: *mut Self::Fields<'db>,
                    new_fields: Self::Fields<'db>,
                ) {
                    use salsa::update::helper::Fallback as _;
                    unsafe {
                        $(
                            $crate::setup_tracked_struct!(@maybe_backdate(
                                $field_option,
                                $field_ty,
                                (*old_fields).$field_index,
                                new_fields.$field_index,
                                revisions[$field_index],
                                current_revision,
                                $zalsa,
                            ));
                        )*
                    }
                }
            }

            impl $Configuration {
                pub fn ingredient<Db>(db: &Db) -> &$zalsa::tracked_struct::Ingredient<Self> {
                    static CACHE: $zalsa::IngredientCache<$zalsa::tracked_struct::Ingredient<Self>> =
                        $zalsa::IngredientCache::new();
                    CACHE.get_or_create(|| {
                        db.add_or_lookup_jar_by_type(&$zalsa::tracked_struct::JarImpl::<$Configuration>)
                    })
                }
            }

            impl<$db_lt, $Db> $zalsa::LookupId<&$db_lt $Db> for $Struct<$db_lt>
            where
                $Db: ?Sized + $zalsa::Database,
            {
                fn lookup_id(id: salsa::Id, db: & $db_lt $Db) -> Self {
                    $Configuration::ingredient(db).lookup_struct(db.runtime(), id)
                }
            }

            impl<$db_lt, $Db> $zalsa::SalsaStructInDb<$Db> for $Struct<$db_lt>
            where
                $Db: ?Sized + $zalsa::Database,
            {
                fn register_dependent_fn(db: & $Db, index: $zalsa::IngredientIndex) {
                    $Configuration::ingredient(db).register_dependent_fn(index)
                }
            }

            impl<$db_lt, $Db> $zalsa::tracked_struct::TrackedStructInDb<#db> for $Struct<$db_lt>
            where
                $Db: ?Sized + $zalsa::Database,
            {
                fn database_key_index(db: &$Db, id: $zalsa::Id) -> $zalsa::DatabaseKeyIndex {
                    $Configuration::ingredient(db).database_key_index(id)
                }
            }

            impl<$db_lt> $Struct<$db_lt> {
                pub fn $new_fn<$Db>(db: &$Db, $($field_id: $field_ty),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + $zalsa::Database,
                {
                    $Configuration::ingredient(db).new_struct(
                        db.runtime(),
                        ($($field_id,)*)
                    )
                }

                $(
                    pub fn $field_id<$Db>(&self, db: &$db_lt $Db) -> &$field_ty
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let runtime = db.runtime();
                        let fields = unsafe { self.0.as_ref() }.field(runtime, $field_index);
                        $crate::setup_tracked_struct!(@maybe_clone(
                            $field_option,
                            $field_ty,
                            &fields.$field_index,
                        ))
                    }
                )*
            }
        };
    };

    // --------------------------------------------------------------
    // @maybe_clone
    //
    // Conditionally invoke `clone` from a field getter

    (
        @maybe_clone(
            (no_clone, $maybe_backdate:ident),
            $field_ty:ty,
            $field_ref_expr:expr,
        )
    ) => {
        $field_ref_expr
    };

    (
        @maybe_clone(
            (clone, $maybe_backdate:ident),
            $field_ty:ty,
            $field_ref_expr:expr,
        )
     ) => {
        <$field_ty as std::clone::Clone>::clone($field_ref_expr)
    };

    // --------------------------------------------------------------
    // @maybe_backdate
    //
    // Conditionally update field value and backdate revisions

    (
        @maybe_backdate(
            ($maybe_clone:ident, no_backdate)
            $field_ty:ty,
            $old_field_place:expr,
            $new_field_place:expr,
            $revision_place:expr,
            $current_revision:expr,
            $zalsa:ident,
        )
    ) => {
        $zalsa::update::always_update(
            &mut $revision_place,
            $current_revision,
            &mut $old_field_place,
            $new_field_place,
        );
    };

    (
        @maybe_backdate(
            ($maybe_clone:ident, backdate),
            $field_ty:ty,
            $old_field_place:expr,
            $new_field_place:expr,
            $revision_place:expr,
            $current_revision:expr,
            $zalsa:ident,
        )
     ) => {
        if $zalsa::update::helper::Dispatch::<$field_ty>::maybe_update(
            $old_field_ptr_expr,
            std::ptr::addr_of_mut!($old_field_place),
            $new_field_place,
        ) {
            $revision_place = #current_revision;
        }
   };
}
