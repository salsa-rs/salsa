use loom::sync::Arc;

#[salsa::interned(debug)]
struct Interned<'db> {
    data: usize,
}

#[test]
fn test_intern_concurrent() {
    loom::model(|| {
        let db = Arc::new(salsa::DatabaseImpl::new());
        let db2 = db.clone();

        let handle = loom::thread::spawn(move || {
            intern(&*db);
        });

        let handle2 = loom::thread::spawn(move || {
            intern(&*db2);
        });

        handle.join().unwrap();
        handle2.join().unwrap();
    });
}

#[salsa::tracked]
fn intern(db: &dyn salsa::Database) -> Interned<'_> {
    Interned::new(db, 0)
}
