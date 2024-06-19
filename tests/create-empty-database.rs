//! Tests that we can create a database of only 0-size jars without invoking UB

use salsa::storage::HasJars;

#[salsa::jar(db = Db)]
struct Jar();

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn execute() {
    let db = Database::default();
    let jars = db.storage.jars().0;

    ensure_init(jars);
}

fn ensure_init(place: *const <Database as HasJars>::Jars) {
    use std::mem::forget;
    use std::ptr::addr_of;

    // SAFETY: Intentionally tries to access potentially uninitialized memory,
    // so that miri can catch if we accidentally forget to initialize the memory.
    #[allow(clippy::forget_non_drop)]
    forget(unsafe { addr_of!((*place).0).read() });
}
