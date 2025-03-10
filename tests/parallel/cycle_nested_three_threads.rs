//! Test a nested-cycle scenario across three threads:
//!
//! ```text
//! Thread T1          Thread T2         Thread T3
//! ---------          ---------         ---------
//!    |                  |                  |
//!    v                  |                  |
//! query_a()             |                  |
//!  ^  |                 v                  |
//!  |  +------------> query_b()             |
//!  |                  ^   |                v
//!  |                  |   +------------> query_c()
//!  |                  |                    |
//!  +------------------+--------------------+
//!
//! ```

use salsa::CycleRecoveryAction;

use crate::setup::{Knobs, KnobsDatabase};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, salsa::Update)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MAX: CycleValue = CycleValue(3);

// Signal 1: T1 has entered `query_a`
// Signal 2: T2 has entered `query_b`
// Signal 3: T3 has entered `query_c`

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_a(db: &dyn KnobsDatabase) -> CycleValue {
    db.signal(1);
    db.wait_for(3);

    query_b(db)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_b(db: &dyn KnobsDatabase) -> CycleValue {
    db.wait_for(1);
    db.signal(2);
    db.wait_for(3);

    let c_value = query_c(db);
    CycleValue(c_value.0 + 1).min(MAX)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_c(db: &dyn KnobsDatabase) -> CycleValue {
    db.wait_for(2);
    db.signal(3);

    let a_value = query_a(db);
    let b_value = query_b(db);
    CycleValue(a_value.0.max(b_value.0))
}

fn cycle_fn(
    _db: &dyn KnobsDatabase,
    _value: &CycleValue,
    _count: u32,
) -> CycleRecoveryAction<CycleValue> {
    CycleRecoveryAction::Iterate
}

fn initial(_db: &dyn KnobsDatabase) -> CycleValue {
    MIN
}

#[test_log::test]
fn the_test() {
    std::thread::scope(|scope| {
        let db_t1 = Knobs::default();
        let db_t2 = db_t1.clone();
        let db_t3 = db_t1.clone();

        let t1 = scope.spawn(move || query_a(&db_t1));
        let t2 = scope.spawn(move || query_b(&db_t2));
        let t3 = scope.spawn(move || query_c(&db_t3));

        let r_t1 = t1.join().unwrap();
        let r_t2 = t2.join().unwrap();
        let r_t3 = t3.join().unwrap();

        assert_eq!((r_t1, r_t2, r_t3), (MAX, MAX, MAX));
    });
}
