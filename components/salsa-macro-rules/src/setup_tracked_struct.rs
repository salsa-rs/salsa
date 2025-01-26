/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_tracked_struct {
    (
        // Attributes on the function.
        attrs: [$(#[$attr:meta]),*],

        // Visibility of the struct.
        vis: $vis:vis,

        // Name of the struct.
        Struct: $Struct:ident,

        // Name of the `'db` lifetime that the user gave.
        db_lt: $db_lt:lifetime,

        // Name user gave for `new`.
        new_fn: $new_fn:ident,

        // Field names.
        field_ids: [$($field_id:ident),*],

        // Tracked field names.
        tracked_ids: [$($tracked_id:ident),*],

        // Visibility and names of tracked fields.
        tracked_getters: [$($tracked_getter_vis:vis $tracked_getter_id:ident),*],

        // Visibility and names of untracked fields.
        untracked_getters: [$($untracked_getter_vis:vis $untracked_getter_id:ident),*],

        // Field types, may reference `db_lt`.
        field_tys: [$($field_ty:ty),*],

        // Tracked field types.
        tracked_tys: [$($tracked_ty:ty),*],

        // Untracked field types.
        untracked_tys: [$($untracked_ty:ty),*],

        // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
        field_indices: [$($field_index:tt),*],

        // Absolute indices of any tracked fields, relative to all other fields of this struct.
        tracked_indices: [$($tracked_index:tt),*],

        // Absolute indices of any untracked fields.
        untracked_indices: [$($untracked_index:tt),*],

        // A set of "field options" for each field.
        //
        // Each field option is a tuple `(maybe_clone, maybe_backdate)` where:
        //
        // * `maybe_clone` is either the identifier `clone` or `no_clone`
        // * `maybe_backdate` is either the identifier `backdate` or `no_backdate`
        //
        // These are used to drive conditional logic for each field via recursive macro invocation
        // (see e.g. @maybe_clone below).
        field_options: [$($field_option:tt),*],

        // A set of "field options" for each tracked field.
        tracked_options: [$($tracked_option:tt),*],

        // A set of "field options" for each untracked field.
        untracked_options: [$($untracked_option:tt),*],

        // Number of fields.
        num_fields: $N:literal,

        // If true, generate a debug impl.
        generate_debug_impl: $generate_debug_impl:tt,

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
        ]
    ) => {
        $(#[$attr])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $Struct<$db_lt>(
            salsa::Id,
            std::marker::PhantomData < & $db_lt salsa::plumbing::tracked_struct::Value < $Struct<'static> > >
        );

        #[allow(clippy::all)]
        #[allow(dead_code)]
        const _: () = {
            use salsa::plumbing as $zalsa;
            use $zalsa::tracked_struct as $zalsa_struct;
            use $zalsa::Revision as $Revision;
            use std::ptr::NonNull as $NonNull;

            type $Configuration = $Struct<'static>;

            impl $zalsa_struct::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($Struct);

                const FIELD_DEBUG_NAMES: &'static [&'static str] = &[
                    $(stringify!($field_id),)*
                ];

                const TRACKED_FIELD_DEBUG_NAMES: &'static [&'static str] = &[
                    $(stringify!($tracked_id),)*
                ];

                type Fields<$db_lt> = ($($field_ty,)*);

                type Revisions = $zalsa::Array<$Revision, $N>;

                type Struct<$db_lt> = $Struct<$db_lt>;

                fn struct_from_id<$db_lt>(id: salsa::Id) -> Self::Struct<$db_lt> {
                    $Struct(id, std::marker::PhantomData)
                }

                fn deref_struct(s: Self::Struct<'_>) -> salsa::Id {
                    s.0
                }

                fn untracked_fields(fields: &Self::Fields<'_>) -> impl std::hash::Hash {
                    ( $( &fields.$untracked_index ),* )
                }

                fn new_revisions(current_revision: $Revision) -> Self::Revisions {
                    $zalsa::Array::new([current_revision; $N])
                }

                unsafe fn update_fields<$db_lt>(
                    current_revision: $Revision,
                    revisions: &mut Self::Revisions,
                    old_fields: *mut Self::Fields<$db_lt>,
                    new_fields: Self::Fields<$db_lt>,
                ) {
                    use $zalsa::UpdateFallback as _;
                    unsafe {
                        $(
                            $crate::maybe_backdate!(
                                $field_option,
                                $field_ty,
                                (*old_fields).$field_index,
                                new_fields.$field_index,
                                revisions[$field_index],
                                current_revision,
                                $zalsa,
                            );
                        )*
                    }
                }
            }

            impl $Configuration {
                pub fn ingredient(db: &dyn $zalsa::Database) -> &$zalsa_struct::IngredientImpl<$Configuration> {
                    static CACHE: $zalsa::IngredientCache<$zalsa_struct::IngredientImpl<$Configuration>> =
                        $zalsa::IngredientCache::new();

                    CACHE.get_or_create(db, || {
                        db.zalsa().add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl::<$Configuration>>::default())
                    })
                }
            }

            impl<$db_lt> $zalsa::FromId for $Struct<$db_lt> {
                fn from_id(id: salsa::Id) -> Self {
                    $Struct(id, std::marker::PhantomData)
                }
            }

            impl $zalsa::AsId for $Struct<'_> {
                fn as_id(&self) -> $zalsa::Id {
                    self.0
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct<'_> {
                fn lookup_ingredient_index(aux: &dyn $zalsa::JarAux) -> core::option::Option<$zalsa::IngredientIndex> {
                    aux.lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default())
                }
            }

            impl $zalsa::TrackedStructInDb for $Struct<'_> {
                fn database_key_index(db: &dyn $zalsa::Database, id: $zalsa::Id) -> $zalsa::DatabaseKeyIndex {
                    $Configuration::ingredient(db).database_key_index(id)
                }
            }

            unsafe impl Send for $Struct<'_> {}

            unsafe impl Sync for $Struct<'_> {}

            $zalsa::macro_if! { $generate_debug_impl =>
                impl std::fmt::Debug for $Struct<'_> {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        Self::default_debug_fmt(*self, f)
                    }
                }
            }

            unsafe impl $zalsa::Update for $Struct<'_> {
                unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
                    if unsafe { *old_pointer } != new_value {
                        unsafe { *old_pointer = new_value };
                        true
                    } else {
                        false
                    }
                }
            }

            impl<$db_lt> $Struct<$db_lt> {
                pub fn $new_fn<$Db>(db: &$db_lt $Db, $($field_id: $field_ty),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + $zalsa::Database,
                {
                    $Configuration::ingredient(db.as_dyn_database()).new_struct(
                        db.as_dyn_database(),
                        ($($field_id,)*)
                    )
                }

                $(
                    $tracked_getter_vis fn $tracked_getter_id<$Db>(self, db: &$db_lt $Db) -> $crate::maybe_cloned_ty!($tracked_option, $db_lt, $tracked_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let db = db.as_dyn_database();
                        let fields = $Configuration::ingredient(db).tracked_field(db, self, $tracked_index);
                        $crate::maybe_clone!(
                            $tracked_option,
                            $tracked_ty,
                            &fields.$tracked_index,
                        )
                    }
                )*

                $(
                    $untracked_getter_vis fn $untracked_getter_id<$Db>(self, db: &$db_lt $Db) -> $crate::maybe_cloned_ty!($untracked_option, $db_lt, $untracked_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let db = db.as_dyn_database();
                        let fields = $Configuration::ingredient(db).untracked_field(db, self, $untracked_index);
                        $crate::maybe_clone!(
                            $untracked_option,
                            $untracked_ty,
                            &fields.$untracked_index,
                        )
                    }
                )*

                /// Default debug formatting for this struct (may be useful if you define your own `Debug` impl)
                pub fn default_debug_fmt(this: Self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    $zalsa::with_attached_database(|db| {
                        let fields = $Configuration::ingredient(db).leak_fields(db, this);
                        let mut f = f.debug_struct(stringify!($Struct));
                        let f = f.field("[salsa id]", &$zalsa::AsId::as_id(&this));
                        $(
                            let f = f.field(stringify!($field_id), &fields.$field_index);
                        )*
                        f.finish()
                    }).unwrap_or_else(|| {
                        f.debug_struct(stringify!($Struct))
                            .field("[salsa id]", &$zalsa::AsId::as_id(&this))
                            .finish()
                    })
                }
            }
        };
    };
}
