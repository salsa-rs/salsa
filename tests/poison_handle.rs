use std::panic::{self, AssertUnwindSafe};

salsa::query_group! {
    trait PoisonHandleDatabase: salsa::Database {
        fn input() -> usize {
            type Input;
            storage input;
        }

        fn panic_if_not_one() -> () {
            type PanicIfNotOne;
        }
    }
}

fn panic_if_not_one(db: &impl PoisonHandleDatabase) -> () {
    assert_eq!(db.input(), 1);
}

#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseStruct> {
        &self.runtime
    }
}

salsa::database_storage! {
    struct DatabaseStorage for DatabaseStruct {
        impl PoisonHandleDatabase {
            fn input() for Input;
            fn panic_if_not_one() for PanicIfNotOne;
        }
    }
}

#[test]
#[should_panic(expected = "attempted to use a poisoned database")]
fn should_poison_handle() {
    let db = DatabaseStruct::default();

    // Invoke `db.panic_if_not_one() without having set `db.input`. `db.input`
    // will default to 0 and we should catch the panic.
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_if_not_one()));
    assert!(result.is_err());

    // The database has been poisoned and any attempt to use it should panic
    db.panic_if_not_one();
}
