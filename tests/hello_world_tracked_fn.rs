//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

// #[salsa::tracked]
// fn identity(db: &dyn Db, input: u32) -> u32 {
//     db.push_log(format!("final_result({:?})", input));
//     input
// }

fn identity(db: &dyn Db, input: u32) -> u32 {
    use salsa::plumbing as zalsa;

    struct Configuration;

    #[derive(Copy, Clone)]
    struct InternedData<'db>(
        std::ptr::NonNull<zalsa::interned::ValueStruct<Configuration>>,
        std::marker::PhantomData<&'db zalsa::interned::ValueStruct<Configuration>>,
    );

    static FN_CACHE: zalsa::IngredientCache<zalsa::function::IngredientImpl<Configuration>> =
        zalsa::IngredientCache::new();

    static INTERN_CACHE: zalsa::IngredientCache<zalsa::interned::IngredientImpl<Configuration>> =
        zalsa::IngredientCache::new();

    impl zalsa::SalsaStructInDb<dyn Db> for InternedData<'_> {
        fn register_dependent_fn(_db: &dyn Db, _index: zalsa::IngredientIndex) {}
    }

    impl zalsa::function::Configuration for Configuration {
        const DEBUG_NAME: &'static str = "identity";

        type DbView = dyn Db;

        type SalsaStruct<'db> = InternedData<'db>;

        type Input<'db> = u32;

        type Output<'db> = u32;

        const CYCLE_STRATEGY: zalsa::CycleRecoveryStrategy = zalsa::CycleRecoveryStrategy::Panic;

        fn should_backdate_value(
            old_value: &Self::Output<'_>,
            new_value: &Self::Output<'_>,
        ) -> bool {
            old_value == new_value
        }

        fn execute<'db>(db: &'db Self::DbView, input: u32) -> Self::Output<'db> {
            fn inner(db: &dyn Db, input: u32) -> u32 {
                db.push_log(format!("final_result({:?})", input));
                input
            }

            inner(db, input)
        }

        fn recover_from_cycle<'db>(
            _db: &'db Self::DbView,
            _cycle: &zalsa::Cycle,
            _key: zalsa::Id,
        ) -> Self::Output<'db> {
            panic!()
        }

        fn id_to_input<'db>(db: &'db Self::DbView, key: salsa::Id) -> Self::Input<'db> {
            let ingredient = INTERN_CACHE.get_or_create(db.as_salsa_database(), || {
                db.add_or_lookup_jar_by_type(&Configuration) + 1
            });
            ingredient.data(key).clone()
        }
    }

    impl zalsa::interned::Configuration for Configuration {
        const DEBUG_NAME: &'static str = "Configuration";

        type Data<'db> = u32;

        type Struct<'db> = InternedData<'db>;

        unsafe fn struct_from_raw<'db>(
            ptr: std::ptr::NonNull<zalsa::interned::ValueStruct<Self>>,
        ) -> Self::Struct<'db> {
            InternedData(ptr, std::marker::PhantomData)
        }

        fn deref_struct(s: Self::Struct<'_>) -> &zalsa::interned::ValueStruct<Self> {
            unsafe { s.0.as_ref() }
        }
    }

    impl zalsa::Jar for Configuration {
        fn create_ingredients(
            &self,
            first_index: zalsa::IngredientIndex,
        ) -> Vec<Box<dyn zalsa::Ingredient>> {
            vec![
                Box::new(<zalsa::function::IngredientImpl<Configuration>>::new(
                    first_index,
                )),
                Box::new(<zalsa::interned::IngredientImpl<Configuration>>::new(
                    first_index + 1,
                )),
            ]
        }
    }

    let fn_ingredient = FN_CACHE.get_or_create(db.as_salsa_database(), || {
        db.add_or_lookup_jar_by_type(&Configuration)
    });
    let intern_ingredient = INTERN_CACHE.get_or_create(db.as_salsa_database(), || {
        db.add_or_lookup_jar_by_type(&Configuration) + 1
    });

    let key = intern_ingredient.intern_id(db.runtime(), input);

    fn_ingredient.fetch(db, key).clone()
}
