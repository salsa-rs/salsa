/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_interned_struct {
    (
        // Attributes on the struct
        attrs: [$(#[$attr:meta]),*],

        // Visibility of the struct
        vis: $vis:vis,

        // Name of the struct
        Struct: $Struct:ident,

        // Name of the `'db` lifetime that the user gave
        db_lt: $db_lt:lifetime,

        // Name user gave for `new`
        new_fn: $new_fn:ident,

        // A series of option tuples; see `setup_tracked_struct` macro
        field_options: [$($field_option:tt),*],

        // Field names
        field_ids: [$($field_id:ident),*],

        // Names for field setter methods (typically `set_foo`)
        field_getters: [$($field_getter_vis:vis $field_getter_id:ident),*],

        // Field types
        field_tys: [$($field_ty:ty),*],

        // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
        field_indices: [$($field_index:tt),*],

        // Number of fields
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
        ]
    ) => {
        $(#[$attr])*
        #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
        $vis struct $Struct<$db_lt>(
            std::ptr::NonNull<salsa::plumbing::interned::Value < $Struct<'static> >>,
            std::marker::PhantomData < & $db_lt salsa::plumbing::interned::Value < $Struct<'static> > >
        );

        const _: () = {
            use salsa::plumbing as $zalsa;
            use $zalsa::interned as $zalsa_struct;

            type $Configuration = $Struct<'static>;

            impl $zalsa_struct::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($Struct);
                type Data<$db_lt> = ($($field_ty,)*);
                type Struct<$db_lt> = $Struct<$db_lt>;
                unsafe fn struct_from_raw<'db>(ptr: std::ptr::NonNull<$zalsa_struct::Value<Self>>) -> Self::Struct<'db> {
                    $Struct(ptr, std::marker::PhantomData)
                }
                fn deref_struct(s: Self::Struct<'_>) -> &$zalsa_struct::Value<Self> {
                    unsafe { s.0.as_ref() }
                }
            }

            impl $Configuration {
                pub fn ingredient<Db>(db: &Db) -> &$zalsa_struct::IngredientImpl<Self>
                where
                    Db: ?Sized + $zalsa::Database,
                {
                    static CACHE: $zalsa::IngredientCache<$zalsa_struct::IngredientImpl<$Configuration>> =
                        $zalsa::IngredientCache::new();
                    CACHE.get_or_create(db.as_dyn_database(), || {
                        db.zalsa().add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default())
                    })
                }
            }

            impl $zalsa::AsId for $Struct<'_> {
                fn as_id(&self) -> salsa::Id {
                    unsafe { self.0.as_ref() }.as_id()
                }
            }

            impl<$db_lt> $zalsa::LookupId<$db_lt> for $Struct<$db_lt> {
                fn lookup_id(id: salsa::Id, db: &$db_lt dyn $zalsa::Database) -> Self {
                    $Configuration::ingredient(db).interned_value(id)
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

            impl $zalsa::SalsaStructInDb for $Struct<'_> {
                fn register_dependent_fn(_db: &dyn $zalsa::Database, _index: $zalsa::IngredientIndex) {
                    // Inputs don't bother with dependent functions
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
                    $Db: ?Sized + salsa::Database,
                {
                    let current_revision = $zalsa::current_revision(db);
                    $Configuration::ingredient(db).intern(db.as_dyn_database(), ($($field_id,)*))
                }

                $(
                    $field_getter_vis fn $field_getter_id<$Db>(self, db: &'db $Db) -> $zalsa::maybe_cloned_ty!($field_option, 'db, $field_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let fields = $Configuration::ingredient(db).fields(self);
                        $zalsa::maybe_clone!(
                            $field_option,
                            $field_ty,
                            &fields.$field_index,
                        )
                    }
                )*

                /// Default debug formatting for this struct (may be useful if you define your own `Debug` impl)
                pub fn default_debug_fmt(this: Self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    $zalsa::with_attached_database(|db| {
                        let fields = $Configuration::ingredient(db).fields(this);
                        let mut f = f.debug_struct(stringify!($Struct));
                        $(
                            let f = f.field(stringify!($field_id), &fields.$field_index);
                        )*
                        f.finish()
                    }).unwrap_or_else(|| {
                        f.debug_tuple(stringify!($Struct))
                            .field(&$zalsa::AsId::as_id(&this))
                            .finish()
                    })
                }
            }
        };
    };
}
