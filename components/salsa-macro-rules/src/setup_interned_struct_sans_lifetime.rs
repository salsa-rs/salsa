/// Macro for setting up a function that must intern its arguments, without any lifetimes.
#[macro_export]
macro_rules! setup_interned_struct_sans_lifetime {
    (
        // Attributes on the struct
        attrs: [$(#[$attr:meta]),*],

        // Visibility of the struct
        vis: $vis:vis,

        // Name of the struct
        Struct: $Struct:ident,

        // Name of the `'db` lifetime that the user gave
        db_lt: $db_lt:lifetime,

        // the salsa ID
        id: $Id:path,

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

        // Indexed types for each field (T0, T1, ...)
        field_indexed_tys: [$($indexed_ty:ident),*],

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
        $vis struct $Struct(
            $Id,
            std::marker::PhantomData < &'static  salsa::plumbing::interned::Value < $Struct > >
        );

        const _: () = {
            use salsa::plumbing as $zalsa;
            use $zalsa::interned as $zalsa_struct;

            type $Configuration = $Struct;

            type StructData<$db_lt> = ($($field_ty,)*);

            /// Key to use during hash lookups. Each field is some type that implements `Lookup<T>`
            /// for the owned type. This permits interning with an `&str` when a `String` is required and so forth.
            struct StructKey<$db_lt, $($indexed_ty: $zalsa::interned::Lookup<$field_ty>),*>(
                $($indexed_ty,)*
                std::marker::PhantomData<&$db_lt ()>,
            );

            impl<$db_lt, $($indexed_ty: $zalsa::interned::Lookup<$field_ty>),*> $zalsa::interned::Lookup<StructData<$db_lt>>
                for StructKey<$db_lt, $($indexed_ty),*> {

                fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
                    $($zalsa::interned::Lookup::hash(&self.$field_index, &mut *h);)*
                }

                fn eq(&self, data: &StructData<$db_lt>) -> bool {
                    ($($zalsa::interned::Lookup::eq(&self.$field_index, &data.$field_index) && )* true)
                }

                #[allow(unused_unit)]
                fn into_owned(self) -> StructData<$db_lt> {
                    ($($zalsa::interned::Lookup::into_owned(self.$field_index),)*)
                }
            }

            impl $zalsa_struct::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($Struct);
                type Data<'a> = StructData<'a>;
                type Struct<'a> = $Struct;
                fn struct_from_id<'db>(id: salsa::Id) -> Self::Struct<'db> {
                    use $zalsa::FromId;
                    $Struct(<$Id>::from_id(id), std::marker::PhantomData)
                }
                fn deref_struct(s: Self::Struct<'_>) -> salsa::Id {
                    use $zalsa::AsId;
                    s.0.as_id()
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

            impl $zalsa::AsId for $Struct {
                fn as_id(&self) -> salsa::Id {
                    self.0.as_id()
                }
            }

            impl $zalsa::FromId for $Struct {
                fn from_id(id: salsa::Id) -> Self {
                    Self(<$Id>::from_id(id), std::marker::PhantomData)
                }
            }

            unsafe impl Send for $Struct {}

            unsafe impl Sync for $Struct {}

            $zalsa::macro_if! { $generate_debug_impl =>
                impl std::fmt::Debug for $Struct {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        Self::default_debug_fmt(*self, f)
                    }
                }
            }

            impl $zalsa::SalsaStructInDb for $Struct {
            }

            unsafe impl $zalsa::Update for $Struct {
                unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
                    if unsafe { *old_pointer } != new_value {
                        unsafe { *old_pointer = new_value };
                        true
                    } else {
                        false
                    }
                }
            }

            impl<$db_lt> $Struct {
                pub fn $new_fn<$Db>(db: &$db_lt $Db,  $($field_id: impl $zalsa::interned::Lookup<$field_ty>),*) -> Self
                where
                    // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                    $Db: ?Sized + salsa::Database,
                {
                    let current_revision = $zalsa::current_revision(db);
                    $Configuration::ingredient(db).intern(db.as_dyn_database(),
                        StructKey::<$db_lt>($($field_id,)* std::marker::PhantomData::default()))
                }

                $(
                    $field_getter_vis fn $field_getter_id<$Db>(self, db: &'db $Db) -> $zalsa::maybe_cloned_ty!($field_option, 'db, $field_ty)
                    where
                        // FIXME(rust-lang/rust#65991): The `db` argument *should* have the type `dyn Database`
                        $Db: ?Sized + $zalsa::Database,
                    {
                        let fields = $Configuration::ingredient(db).fields(db.as_dyn_database(), self);
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
                        let fields = $Configuration::ingredient(db).fields(db.as_dyn_database(), this);
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
