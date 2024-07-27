/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_input_struct {
    (
        // Attributes on the struct
        attrs: [$(#[$attr:meta]),*],

        // Visibility of the struct
        vis: $vis:vis,

        // Name of the struct
        Struct: $Struct:ident,

        // Name user gave for `new`
        new_fn: $new_fn:ident,

        // A series of option tuples; see `setup_tracked_struct` macro
        field_options: [$($field_option:tt),*],

        // Field names
        field_ids: [$($field_id:ident),*],

        // Names for field getter methods (typically `foo`)
        field_getters: [$($field_getter_vis:vis $field_getter_id:ident),*],

        // Names for field setter methods (typically `set_foo`)
        field_setters: [$($field_setter_vis:vis $field_setter_id:ident),*],

        // Field types
        field_tys: [$($field_ty:ty),*],

        // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
        field_indices: [$($field_index:tt),*],

        // Number of fields
        num_fields: $N:literal,

        // If true, this is a singleton input.
        is_singleton: $is_singleton:tt,

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
        $vis struct $Struct(salsa::Id);

        const _: () = {
            use salsa::plumbing as $zalsa;
            use $zalsa::input as $zalsa_struct;

            struct $Configuration;

            impl $zalsa_struct::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($Struct);
                const FIELD_DEBUG_NAMES: &'static [&'static str] = &[$(stringify!($field_id)),*];
                const IS_SINGLETON: bool = $is_singleton;

                /// The input struct (which wraps an `Id`)
                type Struct = $Struct;

                /// A (possibly empty) tuple of the fields for this struct.
                type Fields = ($($field_ty,)*);

                /// A array of [`StampedValue<()>`](`StampedValue`) tuples, one per each of the value fields.
                type Stamps = $zalsa::Array<$zalsa::Stamp, $N>;
            }

            impl $Configuration {
                pub fn ingredient(db: &dyn $zalsa::Database) -> &$zalsa_struct::IngredientImpl<Self> {
                    static CACHE: $zalsa::IngredientCache<$zalsa_struct::IngredientImpl<$Configuration>> =
                        $zalsa::IngredientCache::new();
                    CACHE.get_or_create(db, || {
                        db.zalsa().add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default())
                    })
                }

                pub fn ingredient_mut(db: &mut dyn $zalsa::Database) -> (&mut $zalsa_struct::IngredientImpl<Self>, $zalsa::Revision) {
                    let zalsa_mut = db.zalsa_mut();
                    let index = zalsa_mut.add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default());
                    let current_revision = zalsa_mut.current_revision();
                    let ingredient = zalsa_mut.lookup_ingredient_mut(index);
                    let ingredient = ingredient.assert_type_mut::<$zalsa_struct::IngredientImpl<Self>>();
                    (ingredient, current_revision)
                }
            }

            impl $zalsa::FromId for $Struct {
                fn from_id(id: salsa::Id) -> Self {
                    Self(id)
                }
            }

            impl $zalsa::AsId for $Struct {
                fn as_id(&self) -> salsa::Id {
                    self.0
                }
            }

            $zalsa::macro_if! { $generate_debug_impl =>
                impl std::fmt::Debug for $Struct {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        Self::default_debug_fmt(*self, f)
                    }
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct {
                fn register_dependent_fn(_db: &dyn $zalsa::Database, _index: $zalsa::IngredientIndex) {
                    // Inputs don't bother with dependent functions
                }
            }

            impl $Struct {
                pub fn $new_fn<$Db>(db: &$Db, $($field_id: $field_ty),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + salsa::Database,
                {
                    let current_revision = $zalsa::current_revision(db);
                    let stamps = $zalsa::Array::new([$zalsa::stamp(current_revision, Default::default()); $N]);
                    $Configuration::ingredient(db.as_dyn_database()).new_input(($($field_id,)*), stamps)
                }

                $(
                    $field_getter_vis fn $field_getter_id<'db, $Db>(self, db: &'db $Db) -> $zalsa::maybe_cloned_ty!($field_option, 'db, $field_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let fields = $Configuration::ingredient(db.as_dyn_database()).field(
                            db.as_dyn_database(),
                            self,
                            $field_index,
                        );
                        $zalsa::maybe_clone!(
                            $field_option,
                            $field_ty,
                            &fields.$field_index,
                        )
                    }
                )*

                $(
                    #[must_use]
                    $field_setter_vis fn $field_setter_id<'db, $Db>(self, db: &'db mut $Db) -> impl salsa::Setter<FieldTy = $field_ty> + 'db
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let (ingredient, runtime) = $Configuration::ingredient_mut(db.as_dyn_database_mut());
                        $zalsa::input::SetterImpl::new(
                            runtime,
                            self,
                            $field_index,
                            ingredient,
                            |fields, f| std::mem::replace(&mut fields.$field_index, f),
                        )
                    }
                )*

                $zalsa::macro_if! { $is_singleton =>
                    pub fn try_get<$Db>(db: &$Db) -> Option<Self>
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + salsa::Database,
                    {
                        $Configuration::ingredient(db.as_dyn_database()).get_singleton_input()
                    }

                    #[track_caller]
                    pub fn get<$Db>(db: &$Db) -> Self
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + salsa::Database,
                    {
                        Self::try_get(db).unwrap()
                    }
                }

                /// Default debug formatting for this struct (may be useful if you define your own `Debug` impl)
                pub fn default_debug_fmt(this: Self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    $zalsa::with_attached_database(|db| {
                        let fields = $Configuration::ingredient(db).leak_fields(this);
                        let mut f = f.debug_struct(stringify!($Struct));
                        let f = f.field("[salsa id]", &$zalsa::AsId::as_id(&this));
                        $(
                            let f = f.field(stringify!($field_id), &fields.$field_index);
                        )*
                        f.finish()
                    }).unwrap_or_else(|| {
                        f.debug_struct(stringify!($Struct))
                            .field("[salsa id]", &this.0)
                            .finish()
                    })
                }
            }
        };
    };
}
