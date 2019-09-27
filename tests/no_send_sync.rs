extern crate salsa;

use std::rc::Rc;

#[salsa::query_group(NoSendSyncStorage)]
trait NoSendSyncDatabase: salsa::Database {
    fn no_send_sync_value(&self, key: bool) -> Rc<bool>;
    fn no_send_sync_key(&self, key: Rc<bool>) -> bool;
}

fn no_send_sync_value(_db: &impl NoSendSyncDatabase, key: bool) -> Rc<bool> {
    Rc::new(key)
}

fn no_send_sync_key(_db: &impl NoSendSyncDatabase, key: Rc<bool>) -> bool {
    *key
}

#[salsa::database(NoSendSyncStorage)]
#[derive(Default)]
struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}

#[test]
fn no_send_sync() {
    let db = DatabaseImpl::default();

    assert_eq!(db.no_send_sync_value(true), Rc::new(true));
    assert_eq!(db.no_send_sync_key(Rc::new(false)), false);
}
