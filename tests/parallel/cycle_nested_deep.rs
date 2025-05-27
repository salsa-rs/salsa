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
use crate::sync::thread;
use crate::{Knobs, KnobsDatabase};

use salsa::CycleRecoveryAction;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, salsa::Update)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MAX: CycleValue = CycleValue(3);

// Signal 1: T1 has entered `query_a`
// Signal 2: T2 has entered `query_b`
// Signal 3: T3 has entered `query_c`

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_a(db: &dyn KnobsDatabase) -> CycleValue {
    query_b(db)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_b(db: &dyn KnobsDatabase) -> CycleValue {
    let c_value = query_c(db);
    CycleValue(c_value.0 + 1).min(MAX)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_c(db: &dyn KnobsDatabase) -> CycleValue {
    let d_value = query_d(db);
    let e_value = query_e(db);
    let b_value = query_b(db);
    let a_value = query_a(db);

    CycleValue(d_value.0.max(e_value.0).max(b_value.0).max(a_value.0))
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_d(db: &dyn KnobsDatabase) -> CycleValue {
    query_c(db)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_e(db: &dyn KnobsDatabase) -> CycleValue {
    query_c(db)
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
    // shuttle::replay(
    crate::sync::check(
        || {
            tracing::info!("New run");
            let db_t1 = Knobs::default();
            let db_t2 = db_t1.clone();
            let db_t3 = db_t1.clone();

            let t1 = thread::spawn(move || {
                let _span =
                    tracing::debug_span!("t1", thread_id = ?thread::current().id()).entered();
                let result = query_a(&db_t1);
                db_t1.signal(1);
                result
            });
            let t2 = thread::spawn(move || {
                let _span =
                    tracing::debug_span!("t2", thread_id = ?thread::current().id()).entered();
                db_t2.wait_for(1);
                query_d(&db_t2)
            });
            let t3 = thread::spawn(move || {
                let _span =
                    tracing::debug_span!("t3", thread_id = ?thread::current().id()).entered();
                db_t3.wait_for(1);
                query_e(&db_t3)
            });

            let r_t1 = t1.join().unwrap();
            let r_t2 = t2.join().unwrap();
            let r_t3 = t3.join().unwrap();

            assert_eq!((r_t1, r_t2, r_t3), (MAX, MAX, MAX));

            tracing::info!("Complete");
        }, //     ,
           //     "
           // 9102ac21f5a392c8f88cc0a27300000000004092244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224491248c23664dbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66d
           // dbb66ddbb66d9324499224499224499224499224499224499224499224499224494a92244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 499224499224499224499224499224499224499224c91629dbb66ddbb66d922449922449d9b6
           // 6ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddb364992942449922449924892
           // 244992244992244992244992244992244992b429dab66ddbb66d4b922449922449b46ddbb66d
           // dbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb6
           // 6ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddb
           // b66ddbb66ddbb66ddbb66ddbb66ddbb66ddbb66ddb2449922449922449922449922449922449
           // 922449922449922449d2b66ddbb66ddbb66ddbb66ddbb66ddbb66ddb962c4992244992244992
           // 2449922449922449b66ddbb64d49922449922449922449922449922449922449922449922449
           // 922449922449924892244992244992244992246ddbb624499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 244992244992244992244992244992244992489124499224498a244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449529224499224492249922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 9224499224499224499224499224499224499224499224499224499224499224499224499224
           // 4992244992244992244992244992244992244992244992244992244992244992244992244992
           // 2449922449922449922449922449922449922449922449922449922449922449922449922449
           // 922449922449922449922449922449922409
           // "
    );
}
