//! This crate defines various `macro_rules` macros
//! used as part of Salsa's internal plumbing.
//!
//! The procedural macros typically emit calls to these
//! `macro_rules` macros.
//!
//! Modifying `macro_rules` macro definitions is generally
//! more ergonomic and also permits true hygiene.

// Macro that generates the body of the cycle recovery function
// for the case where no cycle recovery is possible. This has to be
// a macro because it can take a variadic number of arguments.
#[macro_export]
macro_rules! unexpected_cycle_recovery {
    ($db:ident, $cycle:ident, $($other_inputs:ident),*) => {
        {
            std::mem::drop($db);
            std::mem::drop(($($other_inputs),*));
            panic!("cannot recover from cycle `{:?}`", $cycle)
        }
    }
}

/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_interned_fn {
    (
        // Visibility of the function
        vis: $vis:vis,

        // Name of the function
        fn_name: $fn_name:ident,

        // Name of the `'db` lifetime that the user gave; if they didn't, then defaults to `'db`
        db_lt: $db_lt:lifetime,

        // Path to the database trait that the user's database parameter used
        Db: $Db:path,

        // Name of the database parameter given by the user.
        db: $db:ident,

        // An identifier for each function argument EXCEPT the database.
        // We prefer to use the identifier the user gave, but if the user gave a pattern
        // (e.g., `(a, b): (u32, u32)`) we will synthesize an identifier.
        input_ids: [$($input_id:ident),*],

        // Types of the function arguments (may reference `$generics`).
        input_tys: [$($input_ty:ty),*],

        // Return type of the function (may reference `$generics`).
        output_ty: $output_ty:ty,

        // Function body, may reference identifiers defined in `$input_pats` and the generics from `$generics`
        inner_fn: $inner_fn:item,

        // Path to the cycle recovery function to use.
        cycle_recovery_fn: ($($cycle_recovery_fn:tt)*),

        // Name of cycle recovery strategy variant to use.
        cycle_recovery_strategy: $cycle_recovery_strategy:ident,

        // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
        // We have the procedural macro generate names for those items that are
        // not used elsewhere in the user's code.
        unused_names: [
            $zalsa:ident,
            $Configuration:ident,
            $InternedData:ident,
            $FN_CACHE:ident,
            $INTERN_CACHE:ident,
            $inner:ident,
        ]
    ) => {
        $vis fn $fn_name<$db_lt>(
            $db: &$db_lt dyn $Db,
            $($input_id: $input_ty,)*
        ) -> $output_ty {
            use salsa::plumbing as $zalsa;

            struct $Configuration;

            #[derive(Copy, Clone)]
            struct $InternedData<'db>(
                std::ptr::NonNull<$zalsa::interned::ValueStruct<$Configuration>>,
                std::marker::PhantomData<&'db $zalsa::interned::ValueStruct<$Configuration>>,
            );

            static $FN_CACHE: $zalsa::IngredientCache<$zalsa::function::IngredientImpl<$Configuration>> =
                $zalsa::IngredientCache::new();

            static $INTERN_CACHE: $zalsa::IngredientCache<$zalsa::interned::IngredientImpl<$Configuration>> =
                $zalsa::IngredientCache::new();

            impl $zalsa::SalsaStructInDb<dyn $Db> for $InternedData<'_> {
                fn register_dependent_fn(_db: &dyn $Db, _index: $zalsa::IngredientIndex) {}
            }

            impl $zalsa::function::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($fn_name);

                type DbView = dyn $Db;

                type SalsaStruct<$db_lt> = $InternedData<$db_lt>;

                type Input<$db_lt> = ($($input_ty),*);

                type Output<$db_lt> = $output_ty;

                const CYCLE_STRATEGY: $zalsa::CycleRecoveryStrategy = $zalsa::CycleRecoveryStrategy::$cycle_recovery_strategy;

                fn should_backdate_value(
                    old_value: &Self::Output<'_>,
                    new_value: &Self::Output<'_>,
                ) -> bool {
                    old_value == new_value
                }

                fn execute<'db>($db: &'db Self::DbView, ($($input_id),*): ($($input_ty),*)) -> Self::Output<'db> {
                    $inner_fn

                    $inner($db, $($input_id),*)
                }

                fn recover_from_cycle<'db>(
                    db: &$db_lt dyn $Db,
                    cycle: &$zalsa::Cycle,
                    ($($input_id),*): ($($input_ty),*)
                ) -> Self::Output<'db> {
                    $($cycle_recovery_fn)*(db, cycle, $($input_id),*)
                }

                fn id_to_input<'db>(db: &'db Self::DbView, key: salsa::Id) -> Self::Input<'db> {
                    let ingredient = $INTERN_CACHE.get_or_create(db.as_salsa_database(), || {
                        db.add_or_lookup_jar_by_type(&$Configuration) + 1
                    });
                    ingredient.data(key).clone()
                }
            }

            impl $zalsa::interned::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = "Configuration";

                type Data<$db_lt> = ($($input_ty),*);

                type Struct<$db_lt> = $InternedData<$db_lt>;

                unsafe fn struct_from_raw<'db>(
                    ptr: std::ptr::NonNull<$zalsa::interned::ValueStruct<Self>>,
                ) -> Self::Struct<'db> {
                    $InternedData(ptr, std::marker::PhantomData)
                }

                fn deref_struct(s: Self::Struct<'_>) -> &$zalsa::interned::ValueStruct<Self> {
                    unsafe { s.0.as_ref() }
                }
            }

            impl $zalsa::Jar for $Configuration {
                fn create_ingredients(
                    &self,
                    first_index: $zalsa::IngredientIndex,
                ) -> Vec<Box<dyn $zalsa::Ingredient>> {
                    vec![
                        Box::new(<$zalsa::function::IngredientImpl<$Configuration>>::new(
                            first_index,
                        )),
                        Box::new(<$zalsa::interned::IngredientImpl<$Configuration>>::new(
                            first_index + 1,
                        )),
                    ]
                }
            }

            let intern_ingredient = $INTERN_CACHE.get_or_create($db.as_salsa_database(), || {
                $db.add_or_lookup_jar_by_type(&$Configuration) + 1
            });
            let key = intern_ingredient.intern_id($db.runtime(), ($($input_id),*));

            let fn_ingredient = $FN_CACHE.get_or_create($db.as_salsa_database(), || {
                $db.add_or_lookup_jar_by_type(&$Configuration)
            });
            fn_ingredient.fetch($db, key).clone()
        }
    };
}

#[macro_export]
macro_rules! setup_fn {
    () => {};
}
