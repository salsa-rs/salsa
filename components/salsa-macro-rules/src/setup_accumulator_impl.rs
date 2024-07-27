/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_accumulator_impl {
    (
        // Name of the struct
        Struct: $Struct:ident,

        // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
        // We have the procedural macro generate names for those items that are
        // not used elsewhere in the user's code.
        unused_names: [
            $zalsa:ident,
            $zalsa_struct:ident,
            $CACHE:ident,
            $ingredient:ident,
        ]
    ) => {
        const _: () = {
            use salsa::plumbing as $zalsa;
            use salsa::plumbing::accumulator as $zalsa_struct;

            static $CACHE: $zalsa::IngredientCache<$zalsa_struct::IngredientImpl<$Struct>> =
                $zalsa::IngredientCache::new();

            fn $ingredient(db: &dyn $zalsa::Database) -> &$zalsa_struct::IngredientImpl<$Struct> {
                $CACHE.get_or_create(db, || {
                    db.zalsa().add_or_lookup_jar_by_type(&<$zalsa_struct::JarImpl<$Struct>>::default())
                })
            }

            impl $zalsa::Accumulator for $Struct {
                const DEBUG_NAME: &'static str = stringify!($Struct);

                fn accumulate<Db>(self, db: &Db)
                where
                    Db: ?Sized + $zalsa::Database,
                {
                    let db = db.as_dyn_database();
                    $ingredient(db).push(db, self);
                }
            }
        };
    };
}
