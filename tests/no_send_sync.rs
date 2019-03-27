#[macro_use]
extern crate salsa;

use std::rc::Rc;

// #[derive(Clone, PartialEq, Eq, Debug)]
// struct Dummy;

#[salsa::query_group(NoSendStorage)]
trait NoSendDatabase: salsa::Database {
    fn query(&self, key: ()) -> Rc<bool>;
}

fn query(db: &impl NoSendDatabase, (): ()) -> Rc<bool> {
    Rc::new(true)
}

#[salsa::database(NoSendStorage)]
#[derive(Default)]
struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

#[test]
fn no_send_sync() {
    let mut db = DatabaseImpl::default();

    db.query(());
}
