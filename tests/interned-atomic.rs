#![cfg(all(feature = "inventory", not(feature = "shuttle")))]

//! Readers must not see a reusable slot as valid until its old memos have been cleared.

use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use salsa::plumbing::{AsId, FromId};
use salsa::{Database, Setter};

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl Database for TestDb {}

#[salsa::input]
struct Input {
    #[returns(copy)]
    value: u32,
}

#[salsa::interned(revisions = 1)]
struct Value<'db> {
    #[returns(copy)]
    value: SameShard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, salsa::SalsaValue)]
struct SameShard(u32);

impl Hash for SameShard {
    fn hash<H: Hasher>(&self, state: &mut H) {
        0u8.hash(state);
    }
}

#[test]
fn reuse_publishes_revision_after_memo_cleanup() {
    assert_unpublished_during_cleanup(false);
}

#[test]
fn reuse_publishes_durability_after_memo_cleanup() {
    assert_unpublished_during_cleanup(true);
}

#[test]
fn panic_during_memo_cleanup_leaves_slot_reusable() {
    let panic_once = AtomicBool::new(true);
    let mut db = database(move || {
        if panic_once.swap(false, Ordering::Relaxed) {
            panic!("memo cleanup panic");
        }
    });
    let input = Input::new(&db, 0);
    let old = intern(&db, input);
    let old_id = old.as_id();
    payload(&db, old);
    input.set_value(&mut db).to(1);

    assert!(catch_unwind(AssertUnwindSafe(|| intern(&db, input))).is_err());

    let replacement = intern(&db, input);
    assert_eq!(replacement.as_id(), old_id.next_generation().unwrap());
    assert_eq!(replacement.value(&db), SameShard(1));
    assert_eq!(payload(&db, replacement), "1");
}

fn assert_unpublished_during_cleanup(non_reusable: bool) {
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let mut db = database({
        let entered = entered.clone();
        let resume = resume.clone();
        move || {
            entered.wait();
            resume.wait();
        }
    });
    let input = Input::new(&db, 0);
    let old = intern(&db, input);
    let old_id = old.as_id();
    payload(&db, old);
    input.set_value(&mut db).to(1);

    let writer_db = db.clone();
    let writer = std::thread::spawn(move || {
        let replacement = if non_reusable {
            // Values interned outside queries are non-reusable.
            Value::new(&writer_db, SameShard(1))
        } else {
            intern(&writer_db, input)
        };
        assert_eq!(replacement.value(&writer_db), SameShard(1));
        replacement.as_id()
    });

    entered.wait();
    let reader_db = db.clone();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        // Simulate a stale ID returned by a cached query without validating its producer.
        let stale = Value::from_id(old_id);
        let result = catch_unwind(AssertUnwindSafe(|| stale.value(&reader_db)));
        send.send(result.is_err()).unwrap();
    });

    // The reader must reject the slot even while the writer holds its shard lock. Release
    // the writer on timeout too, so a regression to locking fails instead of deadlocking.
    let rejected = receive.recv_timeout(Duration::from_secs(5));
    resume.wait();
    reader.join().unwrap();
    let new_id = writer.join().unwrap();

    assert!(rejected.expect("reader waited for the shard lock"));
    assert_eq!(new_id, old_id.next_generation().unwrap());
}

fn database(on_discard: impl Fn() + Send + Sync + 'static) -> TestDb {
    TestDb {
        storage: salsa::Storage::new(Some(Box::new(move |event| {
            if matches!(event.kind, salsa::EventKind::DidDiscard { .. }) {
                on_discard();
            }
        }))),
    }
}

#[salsa::tracked(returns(copy))]
fn intern(db: &dyn Database, input: Input) -> Value<'_> {
    Value::new(db, SameShard(input.value(db)))
}

#[salsa::tracked(returns(ref))]
fn payload<'db>(db: &'db dyn Database, value: Value<'db>) -> String {
    value.value(db).0.to_string()
}
