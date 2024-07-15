/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_tracked_struct {
    (
        // Attributes on the function
        attrs: [$(#[$attr:meta]),*],

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

        // A set of "field options". Each field option is a tuple `(maybe_clone, maybe_backdate)` where:
        //
        // * `maybe_clone` is either the identifier `clone` or `no_clone`
        // * `maybe_backdate` is either the identifier `backdate` or `no_backdate`
        //
        // These are used to drive conditional logic for each field via recursive macro invocation
        // (see e.g. @maybe_clone below).
        field_options: [$($field_option:tt),*],

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
        ]
    ) => {
        $(#[$attr])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $Struct<$db_lt>(
            std::ptr::NonNull<salsa::plumbing::tracked_struct::Value < $Struct<'static> >>,
            std::marker::PhantomData < & $db_lt salsa::plumbing::tracked_struct::Value < $Struct<'static> > >
        );

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

                type Fields<$db_lt> = ($($field_ty,)*);

                type Revisions = $zalsa::Array<$Revision, $N>;

                type Struct<$db_lt> = $Struct<$db_lt>;

                unsafe fn struct_from_raw<'db>(ptr: $NonNull<$zalsa_struct::Value<Self>>) -> Self::Struct<'db> {
                    $Struct(ptr, std::marker::PhantomData)
                }

                fn deref_struct(s: Self::Struct<'_>) -> &$zalsa_struct::Value<Self> {
                    unsafe { s.0.as_ref() }
                }

                fn id_fields(fields: &Self::Fields<'_>) -> impl std::hash::Hash {
                    ( $( &fields.$id_field_index ),* )
                }

                fn new_revisions(current_revision: $Revision) -> Self::Revisions {
                    $zalsa::Array::new([current_revision; $N])
                }

                unsafe fn update_fields<'db>(
                    current_revision: $Revision,
                    revisions: &mut Self::Revisions,
                    old_fields: *mut Self::Fields<'db>,
                    new_fields: Self::Fields<'db>,
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
                        db.add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl::<$Configuration>>::default())
                    })
                }
            }

            impl<'db> $zalsa::LookupId<'db> for $Struct<$db_lt> {
                fn lookup_id(id: salsa::Id, db: &'db dyn $zalsa::Database) -> Self {
                    $Configuration::ingredient(db).lookup_struct(db.runtime(), id)
                }
            }

            impl $zalsa::AsId for $Struct<'_> {
                fn as_id(&self) -> $zalsa::Id {
                    unsafe { self.0.as_ref() }.as_id()
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct<'_> {
                fn register_dependent_fn(db: &dyn $zalsa::Database, index: $zalsa::IngredientIndex) {
                    $Configuration::ingredient(db).register_dependent_fn(index)
                }
            }

            impl $zalsa::TrackedStructInDb for $Struct<'_> {
                fn database_key_index(db: &dyn $zalsa::Database, id: $zalsa::Id) -> $zalsa::DatabaseKeyIndex {
                    $Configuration::ingredient(db).database_key_index(id)
                }
            }

            unsafe impl Send for $Struct<'_> {}

            unsafe impl Sync for $Struct<'_> {}

            impl std::fmt::Debug for $Struct<'_> {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    $zalsa::with_attached_database(|db| {
                        let fields = $Configuration::ingredient(db).leak_fields(*self);
                        let mut f = f.debug_struct(stringify!($Struct));
                        $(
                            let f = f.field(stringify!($field_id), &fields.$field_index);
                        )*
                        f.finish()
                    }).unwrap_or_else(|| {
                        f.debug_tuple(stringify!($Struct))
                            .field(&self.0)
                            .finish()
                    })
                }
            }

            impl<$db_lt> $Struct<$db_lt> {
                pub fn $new_fn<$Db>(db: &$db_lt $Db, $($field_id: $field_ty),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + $zalsa::Database,
                {
                    $Configuration::ingredient(db.as_salsa_database()).new_struct(
                        db.runtime(),
                        ($($field_id,)*)
                    )
                }

                $(
                    pub fn $field_id<$Db>(&self, db: &$db_lt $Db) -> $crate::maybe_cloned_ty!($field_option, $db_lt, $field_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let runtime = db.runtime();
                        let fields = unsafe { self.0.as_ref() }.field(runtime, $field_index);
                        $crate::maybe_clone!(
                            $field_option,
                            $field_ty,
                            &fields.$field_index,
                        )
                    }
                )*
            }
        };
    };
}
