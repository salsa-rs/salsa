//! Test that you can implement a query using a `dyn Trait` setup.

use salsa::InternId;

#[salsa::database(InternStorage)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl salsa::ParallelDatabase for Database {
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Database {
            storage: self.storage.snapshot(),
        })
    }
}

#[salsa::query_group(InternStorage)]
trait Intern {
    #[salsa::interned]
    fn intern1(&self, x: String) -> InternId;

    #[salsa::interned]
    fn intern2(&self, x: String, y: String) -> InternId;

    #[salsa::interned]
    fn intern_key(&self, x: String) -> InternKey;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InternKey(InternId);

impl salsa::InternKey for InternKey {
    fn from_intern_id(v: InternId) -> Self {
        InternKey(v)
    }

    fn as_intern_id(&self) -> InternId {
        self.0
    }
}

#[test]
fn test_intern1() {
    let db = Database::default();
    let foo0 = db.intern1(format!("foo"));
    let bar0 = db.intern1(format!("bar"));
    let foo1 = db.intern1(format!("foo"));
    let bar1 = db.intern1(format!("bar"));

    assert_eq!(foo0, foo1);
    assert_eq!(bar0, bar1);
    assert_ne!(foo0, bar0);

    assert_eq!(format!("foo"), db.lookup_intern1(foo0));
    assert_eq!(format!("bar"), db.lookup_intern1(bar0));
}

#[test]
fn test_intern2() {
    let db = Database::default();
    let foo0 = db.intern2(format!("x"), format!("foo"));
    let bar0 = db.intern2(format!("x"), format!("bar"));
    let foo1 = db.intern2(format!("x"), format!("foo"));
    let bar1 = db.intern2(format!("x"), format!("bar"));

    assert_eq!(foo0, foo1);
    assert_eq!(bar0, bar1);
    assert_ne!(foo0, bar0);

    assert_eq!((format!("x"), format!("foo")), db.lookup_intern2(foo0));
    assert_eq!((format!("x"), format!("bar")), db.lookup_intern2(bar0));
}

#[test]
fn test_intern_key() {
    let db = Database::default();
    let foo0 = db.intern_key(format!("foo"));
    let bar0 = db.intern_key(format!("bar"));
    let foo1 = db.intern_key(format!("foo"));
    let bar1 = db.intern_key(format!("bar"));

    assert_eq!(foo0, foo1);
    assert_eq!(bar0, bar1);
    assert_ne!(foo0, bar0);

    assert_eq!(format!("foo"), db.lookup_intern_key(foo0));
    assert_eq!(format!("bar"), db.lookup_intern_key(bar0));
}
