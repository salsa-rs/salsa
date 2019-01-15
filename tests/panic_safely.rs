use salsa::{Database, ParallelDatabase, Snapshot};
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

impl salsa::ParallelDatabase for DatabaseStruct {
    fn snapshot(&self) -> Snapshot<Self> {
        Snapshot::new(DatabaseStruct {
            runtime: self.runtime.snapshot(self),
        })
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
    let mut db = DatabaseStruct::default();

    // Invoke `db.panic_safely() without having set `db.one`. `db.one` will
    // default to 0 and we should catch the panic.
    let result = panic::catch_unwind(AssertUnwindSafe({
        let db = db.snapshot();
        move || db.panic_safely()
    }));
    assert!(result.is_err());

    // Set `db.one` to 1 and assert ok
    db.query_mut(One).set((), 1);
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_safely()));
    assert!(result.is_ok())
}

#[test]
fn storages_are_unwind_safe() {
    fn check_unwind_safe<T: std::panic::UnwindSafe>() {}
    check_unwind_safe::<&DatabaseStruct>();
}
