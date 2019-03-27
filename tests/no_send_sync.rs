#[macro_use]
extern crate salsa;

use std::rc::Rc;

#[salsa::query_group(NoSendSyncStorage)]
trait NoSendSyncDatabase: salsa::Database {
    fn no_send_sync_value(&self, key: bool) -> Rc<bool>;
    fn no_send_sync_key(&self, key: Rc<bool>) -> bool;
}

fn no_send_sync_value(db: &impl NoSendSyncDatabase, key: bool) -> Rc<bool> {
    Rc::new(key)
}

fn no_send_sync_key(db: &impl NoSendSyncDatabase, key: Rc<bool>) -> bool {
    *key
}

#[salsa::database(NoSendSyncStorage)]
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

    assert_eq!(db.no_send_sync_value(true), Rc::new(true));
    assert_eq!(db.no_send_sync_key(Rc::new(false)), false);
}
