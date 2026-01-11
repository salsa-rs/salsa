//! Test a specific cycle scenario:
//!
//! ```text
//! Thread T1          Thread T2
//! ---------          ---------
//!    |                  |
//!    v                  |
//! query_a()             |
//!  ^  |                 v
//!  |  +------------> query_b()
//!  |                    |
//!  +--------------------+
//! ```
use crate::KnobsDatabase;

const FALLBACK_A: u32 = 0b01;
const FALLBACK_B: u32 = 0b10;
const OFFSET_A: u32 = 0b0100;
const OFFSET_B: u32 = 0b1000;

// Signal 1: T1 has entered `query_a`
// Signal 2: T2 has entered `query_b`

#[salsa::tracked(cycle_result=cycle_result_a)]
fn query_a(db: &dyn KnobsDatabase) -> u32 {
    db.signal(1);

    // Wait for Thread T2 to enter `query_b` before we continue.
    db.wait_for(2);

    query_b(db) | OFFSET_A
}

#[salsa::tracked(cycle_result=cycle_result_b)]
fn query_b(db: &dyn KnobsDatabase) -> u32 {
    // Wait for Thread T1 to enter `query_a` before we continue.
    db.wait_for(1);

    db.signal(2);

    query_a(db) | OFFSET_B
}

fn cycle_result_a(_db: &dyn KnobsDatabase, _id: salsa::Id) -> u32 {
    FALLBACK_A
}

fn cycle_result_b(_db: &dyn KnobsDatabase, _id: salsa::Id) -> u32 {
    FALLBACK_B
}

#[test_log::test]
fn the_test() {
    use crate::sync::thread;
    use crate::Knobs;

    crate::sync::check(|| {
        tracing::debug!("Starting new run");
        let db_t1 = Knobs::default();
        let db_t2 = db_t1.clone();

        let t1 = thread::spawn(move || {
            let _span = tracing::debug_span!("t1", thread_id = ?thread::current().id()).entered();
            query_a(&db_t1)
        });
        let t2 = thread::spawn(move || {
            let _span = tracing::debug_span!("t2", thread_id = ?thread::current().id()).entered();
            query_b(&db_t2)
        });

        let (r_t1, r_t2) = (t1.join().unwrap(), t2.join().unwrap());

        // With fixpoint iteration, the cycle head uses its fallback value,
        // while the other query computes its result based on the cycle head's fallback.
        // Which query becomes the cycle head depends on thread scheduling.
        //
        // Case 1: query_b is cycle head
        //   query_a = query_b() | OFFSET_A = FALLBACK_B | OFFSET_A = 2 | 4 = 6
        //   query_b = FALLBACK_B = 2
        //
        // Case 2: query_a is cycle head
        //   query_a = FALLBACK_A = 1
        //   query_b = query_a() | OFFSET_B = FALLBACK_A | OFFSET_B = 1 | 8 = 9
        let valid_results = [
            (FALLBACK_B | OFFSET_A, FALLBACK_B), // query_b is cycle head
            (FALLBACK_A, FALLBACK_A | OFFSET_B), // query_a is cycle head
        ];
        assert!(
            valid_results.contains(&(r_t1, r_t2)),
            "unexpected results: ({r_t1}, {r_t2})"
        );
    });
}
