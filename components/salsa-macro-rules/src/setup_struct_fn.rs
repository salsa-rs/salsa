/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_struct_fn {
    (
        // Attributes on the function
        attrs: [$(#[$attr:meta]),*],

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
        input_id: $input_id:ident,

        // Types of the function arguments (may reference `$generics`).
        input_ty: $input_ty:ty,

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
            $FN_CACHE:ident,
            $inner:ident,
        ]
    ) => {
        #[allow(non_camel_case_types)]
        $vis struct $fn_name {
            _priv: std::convert::Infallible,
        }

        $(#[$attr])*
        $vis fn $fn_name<$db_lt>(
            $db: &$db_lt dyn $Db,
            $input_id: $input_ty,
        ) -> $output_ty {
            use salsa::plumbing as $zalsa;

            struct $Configuration;

            static $FN_CACHE: $zalsa::IngredientCache<$zalsa::function::IngredientImpl<$Configuration>> =
                $zalsa::IngredientCache::new();

            impl $Configuration {
                fn fn_ingredient(db: &dyn $Db) -> &$zalsa::function::IngredientImpl<$Configuration> {
                    $FN_CACHE.get_or_create(db.as_salsa_database(), || {
                        <dyn $Db as $Db>::zalsa_db(db);
                        db.add_or_lookup_jar_by_type(&$Configuration)
                    })
                }
            }

            impl $zalsa::function::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($fn_name);

                type DbView = dyn $Db;

                type SalsaStruct<$db_lt> = $input_ty;

                type Input<$db_lt> = $input_ty;

                type Output<$db_lt> = $output_ty;

                const CYCLE_STRATEGY: $zalsa::CycleRecoveryStrategy = $zalsa::CycleRecoveryStrategy::$cycle_recovery_strategy;

                fn should_backdate_value(
                    old_value: &Self::Output<'_>,
                    new_value: &Self::Output<'_>,
                ) -> bool {
                    $zalsa::should_backdate_value(old_value, new_value)
                }

                fn execute<'db>($db: &'db Self::DbView, $input_id: $input_ty) -> Self::Output<'db> {
                    $inner_fn

                    $inner($db, $input_id)
                }

                fn recover_from_cycle<'db>(
                    db: &$db_lt dyn $Db,
                    cycle: &$zalsa::Cycle,
                    $input_id: $input_ty,
                ) -> Self::Output<'db> {
                    $($cycle_recovery_fn)*(db, cycle, $input_id)
                }

                fn id_to_input<'db>(db: &'db Self::DbView, key: salsa::Id) -> Self::Input<'db> {
                    $zalsa::LookupId::lookup_id(key, db.as_salsa_database())
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
                    ]
                }
            }

            impl $fn_name {
                pub fn accumulated<$db_lt, A: salsa::Accumulator>(
                    $db: &$db_lt dyn $Db,
                    $input_id: $input_ty,
                ) -> Vec<A> {
                    use salsa::plumbing as $zalsa;
                    let key = $zalsa::AsId::as_id(&$input_id);
                    let database_key_index = $Configuration::fn_ingredient($db).database_key_index(key);
                    $zalsa::accumulated_by($db.as_salsa_database(), database_key_index)
                }
            }

            $zalsa::attach_database($db, || {
                $Configuration::fn_ingredient($db).fetch($db, $zalsa::AsId::as_id(&$input_id)).clone()
            })
        }
    };
}
