//! Test a specific cycle scenario:
//!
//! Thread T1 calls A which calls B which calls A.
//!
//! Thread T2 calls C which calls B.
//!
//! The trick is that the call from Thread T2 comes before B has reached a fixed point.
//! We want to be sure that C sees the final value (and blocks until it is complete).

use salsa::CycleRecoveryAction;

use crate::setup::{Knobs, KnobsDatabase};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MID: CycleValue = CycleValue(11);
const MAX: CycleValue = CycleValue(22);

#[salsa::tracked(cycle_fn=query_a_cycle_fn, cycle_initial=query_a_initial)]
fn query_a(db: &dyn KnobsDatabase) -> CycleValue {
    eprintln!("query_a()");
    let b_value = query_b(db);

    eprintln!("query_a: {:?}", b_value);

    // When we reach the mid point, signal stage 1 (unblocking T2)
    // and then wait for T2 to signal stage 2.
    if b_value == MID {
        eprintln!("query_a: signal");
        db.signal(1);
        db.wait_for(2);
    }

    b_value
}

fn query_a_cycle_fn(
    _db: &dyn KnobsDatabase,
    value: &CycleValue,
    count: u32,
) -> CycleRecoveryAction<CycleValue> {
    eprintln!("query_a_cycle_fn({:?}, {:?})", value, count);
    CycleRecoveryAction::Iterate
}

fn query_a_initial(_db: &dyn KnobsDatabase) -> CycleValue {
    MIN
}

#[salsa::tracked]
fn query_b(db: &dyn KnobsDatabase) -> CycleValue {
    eprintln!("query_b()");

    let a_value = query_a(db);

    eprintln!("query_b: {:?}", a_value);

    CycleValue(a_value.0 + 1).min(MAX)
}

#[salsa::tracked]
fn query_c(db: &dyn KnobsDatabase) -> CycleValue {
    eprintln!("query_c()");

    // Wait until T1 has reached MID then execute `query_b`.
    // This shoul block and (due to the configuration on our database) signal stage 2.
    db.wait_for(1);

    eprintln!("query_c: signaled");

    query_b(db)
}

#[test]
fn the_test() {
    eprintln!("hi");
    std::thread::scope(|scope| {
        let db_t1 = Knobs::default();

        let db_t2 = db_t1.clone();
        db_t2.signal_on_will_block.store(2);

        // Thread 1:
        scope.spawn(move || {
            let r = query_a(&db_t1);
            assert_eq!(r, MAX);
        });

        // Thread 2:
        scope.spawn(move || {
            let r = query_c(&db_t2);
            assert_eq!(r, MAX);
        });
    });
}
