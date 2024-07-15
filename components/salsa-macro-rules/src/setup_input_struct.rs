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

        // Names for field setter methods (typically `set_foo`)
        field_setter_ids: [$($field_setter_id:ident),*],

        // Field types
        field_tys: [$($field_ty:ty),*],

        // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
        field_indices: [$($field_index:tt),*],

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
                        db.add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default())
                    })
                }

                pub fn ingredient_mut(db: &mut dyn $zalsa::Database) -> (&mut $zalsa_struct::IngredientImpl<Self>, &mut $zalsa::Runtime) {
                    let index = db.add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Configuration>>::default());
                    let (ingredient, runtime) = db.lookup_ingredient_mut(index);
                    let ingredient = ingredient.assert_type_mut::<$zalsa_struct::IngredientImpl<Self>>();
                    (ingredient, runtime)
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

            impl std::fmt::Debug for $Struct {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    $zalsa::with_attached_database(|db| {
                        let fields = $Configuration::ingredient(db).leak_fields(*self);
                        let mut f = f.debug_struct(stringify!($Struct));
                        let f = f.field("[salsa id]", &self.0.as_u32());
                        $(
                            let f = f.field(stringify!($field_id), &fields.$field_index);
                        )*
                        f.finish()
                    }).unwrap_or_else(|| {
                        f.debug_struct(stringify!($Struct))
                            .field("[salsa id]", &self.0.as_u32())
                            .finish()
                    })
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct {
                fn register_dependent_fn(_db: &dyn $zalsa::Database, _index: $zalsa::IngredientIndex) {
                    // Inputs don't bother with dependent functions
                }
            }

            impl $Struct {
                pub fn new<$Db>(db: &$Db, $($field_id: $field_ty),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + salsa::Database,
                {
                    let current_revision = $zalsa::current_revision(db);
                    let stamps = $zalsa::Array::new([$zalsa::stamp(current_revision, Default::default()); $N]);
                    $Configuration::ingredient(db.as_salsa_database()).new_input(($($field_id,)*), stamps)
                }

                $(
                    pub fn $field_id<'db, $Db>(self, db: &'db $Db) -> $zalsa::maybe_cloned_ty!($field_option, 'db, $field_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let runtime = db.runtime();
                        let fields = $Configuration::ingredient(db.as_salsa_database()).field(runtime, self, $field_index);
                        $zalsa::maybe_clone!(
                            $field_option,
                            $field_ty,
                            &fields.$field_index,
                        )
                    }
                )*

                $(
                    #[must_use]
                    pub fn $field_setter_id<'db, $Db>(self, db: &'db mut $Db) -> impl salsa::Setter<FieldTy = $field_ty> + 'db
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let (ingredient, runtime) = $Configuration::ingredient_mut(db.as_salsa_database_mut());
                        $zalsa::input::SetterImpl::new(
                            runtime,
                            self,
                            $field_index,
                            ingredient,
                            |fields, f| std::mem::replace(&mut fields.$field_index, f),
                        )
                    }
                )*
            }
        };
    };
}
