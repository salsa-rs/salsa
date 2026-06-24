#![cfg(feature = "inventory")]

use salsa::{Cancelled, Database as _, DatabaseImpl, Storage};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TracingEvent, Metadata, Subscriber};

#[salsa::tracked]
fn tracked_with_args(_db: &dyn salsa::Database, left: u32, right: u32) -> u32 {
    left + right
}

#[salsa::input]
struct TraceInput {
    value: u32,
}

#[salsa::tracked]
fn populate_reusable_interned_arguments(db: &dyn salsa::Database, input: TraceInput) {
    input.value(db);
    tracked_with_args(db, 1, 2);
}

#[salsa::interned]
struct InternedValue<'db> {
    value: u32,
}

#[salsa::tracked]
fn cancel_inside_active_query(db: &dyn salsa::Database) -> u32 {
    db.cancellation_token().cancel();
    tracked_with_args(db, 1, 2)
}

thread_local! {
    static OTHER_DATABASE: Cell<*const DatabaseImpl> = const { Cell::new(std::ptr::null()) };
}

#[salsa::tracked]
fn call_other_database_from_active_query(_db: &dyn salsa::Database) -> u32 {
    OTHER_DATABASE.with(|other_database| {
        let other_database = other_database.get();
        assert!(!other_database.is_null());
        // SAFETY: Tests set this pointer immediately before the query call and restore it before
        // either database is dropped.
        tracked_with_args(unsafe { &*other_database }, 1, 2)
    })
}

#[salsa::tracked]
fn switch_database_and_call_original(db: &dyn salsa::Database) -> u32 {
    OTHER_DATABASE.with(|other_database| {
        let other_database = other_database.get();
        assert!(!other_database.is_null());
        // SAFETY: See `call_other_database_from_active_query`.
        salsa::attach_allow_change(unsafe { &*other_database }, || tracked_with_args(db, 1, 2))
    })
}

fn catch_with_other_database<R>(
    other_database: &DatabaseImpl,
    op: impl FnOnce() -> R,
) -> std::thread::Result<R> {
    OTHER_DATABASE.with(|slot| slot.set(other_database));
    let result = catch_unwind(AssertUnwindSafe(op));
    OTHER_DATABASE.with(|slot| slot.set(std::ptr::null()));
    result
}

fn assert_panics_with<R>(result: std::thread::Result<R>, expected: &str) {
    let Err(payload) = result else {
        panic!("expected panic containing {expected:?}");
    };
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    assert!(message.contains(expected), "unexpected panic: {message}");
}

#[salsa::db]
struct EventDatabase {
    storage: Storage<Self>,
}

impl EventDatabase {
    fn new(callback: impl Fn(salsa::Event) + Send + Sync + 'static) -> Self {
        Self {
            storage: Storage::new(Some(Box::new(callback))),
        }
    }
}

#[salsa::db]
impl salsa::Database for EventDatabase {}

#[test]
#[should_panic(expected = "Cannot change database mid-query")]
fn different_database_panics_on_cold_query() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();

    db1.attach(|_| tracked_with_args(&db2, 1, 2));
}

#[test]
#[should_panic(expected = "Cannot change database mid-query")]
fn different_database_panics_on_hot_query() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();
    tracked_with_args(&db2, 1, 2);

    db1.attach(|_| tracked_with_args(&db2, 1, 2));
}

#[test]
fn different_database_panics_on_hot_query_inside_active_query() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();
    tracked_with_args(&db2, 1, 2);

    let result = catch_with_other_database(&db2, || call_other_database_from_active_query(&db1));

    assert_panics_with(result, "Cannot change database mid-query");
}

#[test]
fn cloned_database_panics_on_hot_query_inside_active_query() {
    let db1 = DatabaseImpl::default();
    let db2 = db1.clone();
    tracked_with_args(&db2, 1, 2);

    let result = catch_with_other_database(&db2, || call_other_database_from_active_query(&db1));

    assert_panics_with(result, "Cannot change database mid-query");
}

#[test]
fn switched_database_does_not_match_suspended_active_query() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();
    tracked_with_args(&db1, 1, 2);

    let result = catch_with_other_database(&db2, || switch_database_and_call_original(&db1));

    assert_panics_with(result, "Cannot change database mid-query");
}

#[test]
fn attach_allow_change_restores_attachment_markers_after_panic() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();

    db1.attach(|_| {
        assert_eq!(
            salsa::attach_allow_change(&db1, || tracked_with_args(&db1, 1, 2)),
            3
        );
        assert_eq!(
            salsa::attach_allow_change(&db2, || tracked_with_args(&db2, 1, 2)),
            3
        );
        assert!(
            catch_unwind(AssertUnwindSafe(|| salsa::attach_allow_change(
                &db2,
                || panic!("switch panic")
            )))
            .is_err()
        );

        assert_eq!(tracked_with_args(&db1, 1, 2), 3);
        let result = catch_unwind(AssertUnwindSafe(|| tracked_with_args(&db2, 1, 2)));
        assert_panics_with(result, "Cannot change database mid-query");
    });

    let result = catch_unwind(AssertUnwindSafe(|| {
        db2.attach(|_| tracked_with_args(&db1, 1, 2))
    }));
    assert_panics_with(result, "Cannot change database mid-query");
}

#[test]
fn mismatch_panic_restores_attachment() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();
    tracked_with_args(&db2, 1, 2);

    let result = catch_unwind(AssertUnwindSafe(|| {
        db1.attach(|_| tracked_with_args(&db2, 1, 2))
    }));

    assert!(result.is_err());
    assert!(salsa::with_attached_database(|_| ()).is_none());
    assert_eq!(tracked_with_args(&db1, 1, 2), 3);
    assert_eq!(tracked_with_args(&db2, 1, 2), 3);
}

#[test]
fn different_database_panics_before_direct_interning() {
    let db1 = DatabaseImpl::default();
    let db2 = DatabaseImpl::default();

    let result = catch_unwind(AssertUnwindSafe(|| {
        db1.attach(|_| InternedValue::new(&db2, 1))
    }));

    assert!(result.is_err());
    assert_eq!(InternedValue::new(&db2, 1).value(&db2), 1);
}

#[test]
fn top_level_hot_cancellation_is_cleared() {
    let db = DatabaseImpl::default();
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
    let token = db.cancellation_token();
    token.cancel();

    let result = Cancelled::catch(|| tracked_with_args(&db, 1, 2));

    assert!(matches!(result, Err(Cancelled::Local)), "{result:?}");
    assert!(!token.is_cancelled());
    assert!(salsa::with_attached_database(|_| ()).is_none());
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
}

#[test]
fn outer_attachment_owns_cancellation_cleanup() {
    let db = DatabaseImpl::default();
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
    let token = db.cancellation_token();

    db.attach(|_| {
        token.cancel();
        let result = Cancelled::catch(|| tracked_with_args(&db, 1, 2));
        assert!(matches!(result, Err(Cancelled::Local)), "{result:?}");
        assert!(token.is_cancelled());
    });

    assert!(!token.is_cancelled());
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
}

#[test]
fn active_query_attachment_owns_cancellation_cleanup() {
    let db = DatabaseImpl::default();
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
    let token = db.cancellation_token();

    let result = Cancelled::catch(|| cancel_inside_active_query(&db));

    assert!(matches!(result, Err(Cancelled::Local)), "{result:?}");
    assert!(!token.is_cancelled());
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
}

#[test]
fn event_callback_for_hot_query_has_attached_database() {
    let saw_event = Arc::new(AtomicBool::new(false));
    let db = EventDatabase::new({
        let saw_event = saw_event.clone();
        move |event| {
            if matches!(event.kind, salsa::EventKind::WillCheckCancellation) {
                assert!(salsa::with_attached_database(|_| ()).is_some());
                saw_event.store(true, Ordering::Relaxed);
            }
        }
    });
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
    saw_event.store(false, Ordering::Relaxed);

    assert_eq!(tracked_with_args(&db, 1, 2), 3);

    assert!(saw_event.load(Ordering::Relaxed));
    assert!(salsa::with_attached_database(|_| ()).is_none());
}

#[test]
fn event_panic_on_hot_query_cleans_up_attachment_and_cancellation() {
    let panic_on_event = Arc::new(AtomicBool::new(false));
    let db = EventDatabase::new({
        let panic_on_event = panic_on_event.clone();
        move |event| {
            if matches!(event.kind, salsa::EventKind::WillCheckCancellation)
                && panic_on_event.load(Ordering::Relaxed)
            {
                assert!(salsa::with_attached_database(|_| ()).is_some());
                panic!("event callback panic");
            }
        }
    });
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
    let token = db.cancellation_token();
    token.cancel();
    panic_on_event.store(true, Ordering::Relaxed);

    let result = catch_unwind(AssertUnwindSafe(|| tracked_with_args(&db, 1, 2)));

    assert!(result.is_err());
    assert!(!token.is_cancelled());
    assert!(salsa::with_attached_database(|_| ()).is_none());
    panic_on_event.store(false, Ordering::Relaxed);
    assert_eq!(tracked_with_args(&db, 1, 2), 3);
}

#[derive(Default)]
struct TraceState {
    next_span: AtomicU64,
    saw_attached_key: AtomicBool,
    saw_interned_arguments: AtomicBool,
    saw_raw_key: AtomicBool,
}

struct TraceSubscriber(Arc<TraceState>);

impl TraceSubscriber {
    fn record(&self, record: impl FnOnce(&mut FieldVisitor)) {
        let mut visitor = FieldVisitor(&self.0);
        record(&mut visitor);
    }
}

impl Subscriber for TraceSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        self.record(|visitor| attributes.record(visitor));
        Id::from_u64(self.0.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, values: &Record<'_>) {
        self.record(|visitor| values.record(visitor));
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &TracingEvent<'_>) {
        self.record(|visitor| event.record(visitor));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

struct FieldVisitor<'a>(&'a TraceState);

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let value = format!("{}={value:?}", field.name());
        if value.contains("Id(") {
            assert!(
                salsa::with_attached_database(|_| ()).is_some(),
                "database is not attached while recording {value}"
            );
            self.0.saw_attached_key.store(true, Ordering::Relaxed);
        }
        if value.contains("tracked_with_args::interned_arguments") {
            self.0.saw_interned_arguments.store(true, Ordering::Relaxed);
        }
        if value.contains("DatabaseKeyIndex(") {
            self.0.saw_raw_key.store(true, Ordering::Relaxed);
        }
    }
}

#[test]
fn tracing_hot_query_attaches_database() {
    let db = DatabaseImpl::default();
    let input = TraceInput::new(&db, 0);
    populate_reusable_interned_arguments(&db, input);

    let state = Arc::new(TraceState::default());
    let dispatch = tracing::Dispatch::new(TraceSubscriber(state.clone()));

    tracing::dispatcher::with_default(&dispatch, || {
        tracked_with_args(&db, 1, 2);
    });

    assert!(state.saw_attached_key.load(Ordering::Relaxed));
    assert!(state.saw_interned_arguments.load(Ordering::Relaxed));
    assert!(!state.saw_raw_key.load(Ordering::Relaxed));
}
