//! Test a deeply nested-cycle scenario where cycles have changing query dependencies.
//!
//! The trick is that different threads call into the same cycle from different entry queries and
//! the cycle heads change over different iterations
//!
//! * Thread 1: `a` -> `b` -> `c`
//! * Thread 2: `b`
//! * Thread 3: `d` -> `c`
//! * Thread 4: `e` -> `c`
//!
//! `c` calls:
//! * `d` and `a` in the first few iterations
//! * `d`, `b` and `e` in the last iterations
//!
//! Specifically, the maybe_changed_after flow.
use crate::sync::thread;

use salsa::CycleRecoveryAction;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, salsa::Update)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MAX: CycleValue = CycleValue(3);

#[salsa::input]
struct Input {
    value: u32,
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_a(db: &dyn salsa::Database, input: Input) -> CycleValue {
    query_b(db, input)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_b(db: &dyn salsa::Database, input: Input) -> CycleValue {
    let c_value = query_c(db, input);
    CycleValue(c_value.0 + input.value(db).max(1)).min(MAX)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_c(db: &dyn salsa::Database, input: Input) -> CycleValue {
    let d_value = query_d(db, input);

    if d_value > CycleValue(0) {
        let e_value = query_e(db, input);
        let b_value = query_b(db, input);
        CycleValue(d_value.0.max(e_value.0).max(b_value.0))
    } else {
        let a_value = query_a(db, input);
        CycleValue(d_value.0.max(a_value.0))
    }
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_d(db: &dyn salsa::Database, input: Input) -> CycleValue {
    query_c(db, input)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_e(db: &dyn salsa::Database, input: Input) -> CycleValue {
    query_c(db, input)
}

fn cycle_fn(
    _db: &dyn salsa::Database,
    _value: &CycleValue,
    _count: u32,
    _input: Input,
) -> CycleRecoveryAction<CycleValue> {
    CycleRecoveryAction::Iterate
}

fn initial(_db: &dyn salsa::Database, _input: Input) -> CycleValue {
    MIN
}

#[test_log::test]
fn the_test() {
    use crate::sync;
    use salsa::Setter as _;
    sync::check(|| {
        tracing::debug!("New run");

        // This is a bit silly but it works around https://github.com/awslabs/shuttle/issues/192
        static INITIALIZE: sync::Mutex<Option<(salsa::DatabaseImpl, Input)>> =
            sync::Mutex::new(None);

        fn get_db(f: impl FnOnce(&salsa::DatabaseImpl, Input)) -> (salsa::DatabaseImpl, Input) {
            let mut shared = INITIALIZE.lock().unwrap();

            if let Some((db, input)) = shared.as_ref() {
                return (db.clone(), *input);
            }

            let mut db = salsa::DatabaseImpl::default();

            let input = Input::new(&db, 0);

            f(&db, input);

            input.set_value(&mut db).to(1);

            *shared = Some((db.clone(), input));

            (db, input)
        }

        let t1 = thread::spawn(move || {
            let (db, input) = get_db(|db, input| {
                query_a(db, input);
            });

            let _span = tracing::debug_span!("t1", thread_id = ?thread::current().id()).entered();

            query_a(&db, input)
        });
        let t2 = thread::spawn(move || {
            let (db, input) = get_db(|db, input| {
                query_b(db, input);
            });

            let _span = tracing::debug_span!("t4", thread_id = ?thread::current().id()).entered();
            query_b(&db, input)
        });
        let t3 = thread::spawn(move || {
            let (db, input) = get_db(|db, input| {
                query_d(db, input);
            });

            let _span = tracing::debug_span!("t2", thread_id = ?thread::current().id()).entered();
            query_d(&db, input)
        });
        let t4 = thread::spawn(move || {
            let (db, input) = get_db(|db, input| {
                query_e(db, input);
            });

            let _span = tracing::debug_span!("t3", thread_id = ?thread::current().id()).entered();
            query_e(&db, input)
        });

        let r_t1 = t1.join().unwrap();
        let r_t2 = t2.join().unwrap();
        let r_t3 = t3.join().unwrap();
        let r_t4 = t4.join().unwrap();

        assert_eq!((r_t1, r_t2, r_t3, r_t4), (MAX, MAX, MAX, MAX));
    });
}
