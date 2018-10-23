use salsa::Database;
use std::panic::{self, AssertUnwindSafe};

salsa::query_group! {
    trait PanicSafelyDatabase: salsa::Database {
        fn one() -> usize {
            type One;
            storage input;
        }

        fn panic_safely() -> () {
            type PanicSafely;
        }
    }
}

fn panic_safely(db: &impl PanicSafelyDatabase) -> () {
    assert_eq!(db.one(), 1);
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
        impl PanicSafelyDatabase {
            fn one() for One;
            fn panic_safely() for PanicSafely;
        }
    }
}

#[test]
fn should_panic_safely() {
    let db = DatabaseStruct::default();

    // Invoke `db.panic_safely() without having set `db.one`. `db.one` will
    // default to 0 and we should catch the panic.
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_safely()));
    assert!(result.is_err());

    // Set `db.one` to 1 and assert ok
    db.query(One).set((), 1);
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_safely()));
    assert!(result.is_ok())
}
