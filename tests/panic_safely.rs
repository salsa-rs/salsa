use salsa::{Database, ParallelDatabase, Snapshot};
use std::panic::{self, AssertUnwindSafe};

#[salsa::query_group(PanicSafelyStruct)]
trait PanicSafelyDatabase: salsa::Database {
    #[salsa::input]
    fn one(&self) -> usize;

    fn panic_safely(&self) -> ();
}

fn panic_safely(db: &impl PanicSafelyDatabase) -> () {
    assert_eq!(db.one(), 1);
}

#[salsa::database(PanicSafelyStruct)]
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
    db.set_one(1);
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_safely()));
    assert!(result.is_ok())
}

#[test]
fn storages_are_unwind_safe() {
    fn check_unwind_safe<T: std::panic::UnwindSafe>() {}
    check_unwind_safe::<&DatabaseStruct>();
}

#[test]
fn panics_clear_query_stack() {
    let db = DatabaseStruct::default();

    // Invoke `db.panic_if_not_one() without having set `db.input`. `db.input`
    // will default to 0 and we should catch the panic.
    let result = panic::catch_unwind(AssertUnwindSafe(|| db.panic_safely()));
    assert!(result.is_err());

    // The database has been poisoned and any attempt to increment the
    // revision should panic.
    assert_eq!(db.salsa_runtime().active_query(), None);
}
