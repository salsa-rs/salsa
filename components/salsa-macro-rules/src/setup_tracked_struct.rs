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
        absolute_tracked_indices: [$($absolute_tracked_index:tt),*],

        // Indices of any tracked fields, relative to only tracked fields on this struct.
        relative_tracked_indices: [$($relative_tracked_index:tt),*],

        // Absolute indices of any untracked fields.
        absolute_untracked_indices: [$($absolute_untracked_index:tt),*],

        // A set of "field options" for each tracked field.
        //
        // Each field option is a tuple `(return_mode, maybe_backdate)` where:
        //
        // * `return_mode` is an identifier as specified in `salsa_macros::options::Option::returns`
        // * `maybe_backdate` is either the identifier `backdate` or `no_backdate`
        //
        // These are used to drive conditional logic for each field via recursive macro invocation
        // (see e.g. @return_mode below).
        tracked_options: [$($tracked_option:tt),*],

        // A set of "field options" for each untracked field.
        //
        // Each field option is a tuple `(return_mode, maybe_backdate)` where:
        //
        // * `return_mode` is an identifier as specified in `salsa_macros::options::Option::returns`
        // * `maybe_backdate` is either the identifier `backdate` or `no_backdate`
        //
        // These are used to drive conditional logic for each field via recursive macro invocation
        // (see e.g. @return_mode below).
        untracked_options: [$($untracked_option:tt),*],

        // Attrs for each field.
        tracked_field_attrs: [$([$(#[$tracked_field_attr:meta]),*]),*],
        untracked_field_attrs: [$([$(#[$untracked_field_attr:meta]),*]),*],

        // Number of tracked fields.
        num_tracked_fields: $N:literal,

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
            $Revision:ident,
        ]
    ) => {
        $(#[$attr])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash)]
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

            type $Configuration = $Struct<'static>;

            impl $zalsa_struct::Configuration for $Configuration {
                const LOCATION: $zalsa::Location = $zalsa::Location {
                    file: file!(),
                    line: line!(),
                };
                const DEBUG_NAME: &'static str = stringify!($Struct);

                const TRACKED_FIELD_NAMES: &'static [&'static str] = &[
                    $(stringify!($tracked_id),)*
                ];

                const TRACKED_FIELD_INDICES: &'static [usize] = &[
                    $($relative_tracked_index,)*
                ];

                type Fields<$db_lt> = ($($field_ty,)*);

                type Revisions = [$Revision; $N];

                type Struct<$db_lt> = $Struct<$db_lt>;

                fn untracked_fields(fields: &Self::Fields<'_>) -> impl std::hash::Hash {
                    ( $( &fields.$absolute_untracked_index ),* )
                }

                fn new_revisions(current_revision: $Revision) -> Self::Revisions {
                    [current_revision; $N]
                }

                unsafe fn update_fields<$db_lt>(
                    current_revision: $Revision,
                    revisions: &mut Self::Revisions,
                    old_fields: *mut Self::Fields<$db_lt>,
                    new_fields: Self::Fields<$db_lt>,
                ) -> bool {
                    use $zalsa::UpdateFallback as _;
                    unsafe {
                        $(
                            $crate::maybe_backdate!(
                                $tracked_option,
                                $tracked_ty,
                                (*old_fields).$absolute_tracked_index,
                                new_fields.$absolute_tracked_index,
                                revisions[$relative_tracked_index],
                                current_revision,
                                $zalsa,
                            );
                        )*;

                        // If any untracked field has changed, return `true`, indicating that the tracked struct
                        // itself should be considered changed.
                        $(
                            $zalsa::UpdateDispatch::<$untracked_ty>::maybe_update(
                                &mut (*old_fields).$absolute_untracked_index,
                                new_fields.$absolute_untracked_index,
                            )
                            |
                        )* false
                    }
                }
            }

            impl $Configuration {
                pub fn ingredient(db: &dyn $zalsa::Database) -> &$zalsa_struct::IngredientImpl<Self> {
                    Self::ingredient_(db.zalsa())
                }

                fn ingredient_(zalsa: &$zalsa::Zalsa) -> &$zalsa_struct::IngredientImpl<Self> {
                    static CACHE: $zalsa::IngredientCache<$zalsa_struct::IngredientImpl<$Configuration>> =
                        $zalsa::IngredientCache::new();

                    CACHE.get_or_create(zalsa, || {
                        zalsa.add_or_lookup_jar_by_type::<$zalsa_struct::JarImpl<$Configuration>>()
                    })
                }
            }

            impl<$db_lt> $zalsa::FromId for $Struct<$db_lt> {
                #[inline]
                fn from_id(id: salsa::Id) -> Self {
                    $Struct(id, std::marker::PhantomData)
                }
            }

            impl $zalsa::AsId for $Struct<'_> {
                #[inline]
                fn as_id(&self) -> $zalsa::Id {
                    self.0
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct<'_> {
                type MemoIngredientMap = $zalsa::MemoIngredientSingletonIndex;

                fn lookup_or_create_ingredient_index(aux: &$zalsa::Zalsa) -> $zalsa::IngredientIndices {
                    aux.add_or_lookup_jar_by_type::<$zalsa_struct::JarImpl<$Configuration>>().into()
                }

                #[inline]
                fn cast(id: $zalsa::Id, type_id: $zalsa::TypeId) -> $zalsa::Option<Self> {
                    if type_id == $zalsa::TypeId::of::<$Struct<'static>>() {
                        $zalsa::Some(<$Struct<'static> as $zalsa::FromId>::from_id(id))
                    } else {
                        $zalsa::None
                    }
                }
            }

            impl $zalsa::TrackedStructInDb for $Struct<'_> {
                fn database_key_index(zalsa: &$zalsa::Zalsa, id: $zalsa::Id) -> $zalsa::DatabaseKeyIndex {
                    $Configuration::ingredient_(zalsa).database_key_index(id)
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
                    $(#[$tracked_field_attr])*
                    $tracked_getter_vis fn $tracked_getter_id<$Db>(self, db: &$db_lt $Db) -> $crate::return_mode_ty!($tracked_option, $db_lt, $tracked_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let db = db.as_dyn_database();
                        let fields = $Configuration::ingredient(db).tracked_field(db, self, $relative_tracked_index);
                        $crate::return_mode_expression!(
                            $tracked_option,
                            $tracked_ty,
                            &fields.$absolute_tracked_index,
                        )
                    }
                )*

                $(
                    $(#[$untracked_field_attr])*
                    $untracked_getter_vis fn $untracked_getter_id<$Db>(self, db: &$db_lt $Db) -> $crate::return_mode_ty!($untracked_option, $db_lt, $untracked_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let db = db.as_dyn_database();
                        let fields = $Configuration::ingredient(db).untracked_field(db, self);
                        $crate::return_mode_expression!(
                            $untracked_option,
                            $untracked_ty,
                            &fields.$absolute_untracked_index,
                        )
                    }
                )*
            }

            #[allow(unused_lifetimes)]
            impl<'_db> $Struct<'_db> {
                /// Default debug formatting for this struct (may be useful if you define your own `Debug` impl)
                pub fn default_debug_fmt(this: Self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
                where
                    // `zalsa::with_attached_database` has a local lifetime for the database
                    // so we need this function to be higher-ranked over the db lifetime
                    // Thus the actual lifetime of `Self` does not matter here so we discard
                    // it with the `'_db` lifetime name as we cannot shadow lifetimes.
                    $(for<$db_lt> $field_ty: std::fmt::Debug),*
                {
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
