// Shuttle doesn't like panics inside of its runtime.
#![cfg(not(feature = "shuttle"))]

use std::panic::catch_unwind;

use salsa::{Cancelled, Database};

use crate::setup::{Knobs, KnobsDatabase};
use crate::sync::thread;

#[salsa::input]
struct Input {
    value: u32,
}

/// A query with cycle recovery that will be interrupted by LRU eviction.
/// This uses `cycle_initial` which gives it `FallbackImmediate` cycle recovery strategy,
/// meaning it goes through `execute_maybe_iterate` and has `PoisonProvisionalIfPanicking`.
#[salsa::tracked(cycle_initial = cycle_initial)]
fn cycle_query(db: &dyn KnobsDatabase, input: Input) -> u32 {
    // Signal that we've started the cycle query
    db.signal(1);
    // Wait for signal that the cancellation flag has been set
    db.wait_for(2);
    // This fetch will check for cancellation and throw PendingWrite
    inner_query(db, input)
}

#[salsa::tracked]
fn inner_query(db: &dyn KnobsDatabase, input: Input) -> u32 {
    input.value(db)
}

fn cycle_initial(_db: &dyn KnobsDatabase, _id: salsa::Id, _input: Input) -> u32 {
    0
}

/// Test that `trigger_lru_eviction` during cycle iteration poisons the memo
/// and causes subsequent queries to fail with PropagatedPanic.
#[test]
fn lru_eviction_poisons_cycle_query() {
    let db = Knobs::default();

    // Create clones BEFORE setting up signal handlers
    let db_writer = db.clone();
    let db_t1 = db.clone();
    let db_waiter = db.clone();

    // Create input before setting up signal handlers
    let input = Input::new(&db, 42);

    // Set up: when cancellation flag is set, signal stage 2 so thread 1 can continue
    db.signal_on_did_cancel(2);

    // Drop the original db so trigger_lru_eviction can complete
    // (it waits for all snapshots to be dropped)
    drop(db);

    // Thread 1: Start a cycle query that will be interrupted
    let t1 = thread::spawn(move || catch_unwind(|| cycle_query(&db_t1, input)));

    // Wait for thread 1 to enter the cycle query and signal stage 1
    db_waiter.wait_for(1);

    // Drop waiter so trigger_lru_eviction can proceed after t1 drops its handle
    drop(db_waiter);

    // Thread 2: Trigger LRU eviction, which will:
    // 1. Set the cancellation flag (this signals stage 2, letting thread 1 continue)
    // 2. Wait for thread 1 to drop its snapshot (thread 1 will check cancellation and panic)
    // 3. NOT bump the revision
    let t2 = thread::spawn({
        let mut db_writer = db_writer;
        move || {
            db_writer.trigger_lru_eviction();
            db_writer
        }
    });

    // Thread 1 should have been cancelled with PendingWrite
    let r1 = t1.join().unwrap();
    assert!(
        r1.is_err(),
        "Thread 1 should have panicked due to PendingWrite cancellation"
    );
    let err = r1.unwrap_err();
    assert!(
        err.downcast_ref::<Cancelled>().is_some(),
        "Thread 1 should have been cancelled, got: {:?}",
        err
    );

    // Thread 2 should complete successfully
    let db_after = t2.join().unwrap();

    // Now here's the bug: if we try to query again in the same revision,
    // we'll find the poisoned memo and get PropagatedPanic instead of
    // successfully recomputing the query.
    //
    // The query SHOULD succeed because:
    // - The cancellation is over (flag is reset)
    // - The input hasn't changed
    // - We should just recompute the query
    //
    // But instead, we'll get PropagatedPanic because the poisoned memo
    // has `verified_at == current_revision` and `value == None`.
    let result = catch_unwind(|| cycle_query(&db_after, input));

    // THIS IS THE BUG: The query fails with PropagatedPanic instead of succeeding
    //
    // When the bug is fixed, this assertion should change to:
    // assert!(result.is_ok(), "Query should succeed after cancellation is complete");
    // assert_eq!(result.unwrap(), 42);
    assert!(
        result.is_err(),
        "BUG: Query fails with PropagatedPanic due to poisoned memo"
    );
    let err = result.unwrap_err();
    let is_propagated_panic = err
        .downcast_ref::<Cancelled>()
        .is_some_and(|c| matches!(c, Cancelled::PropagatedPanic));
    assert!(
        is_propagated_panic,
        "BUG: Should get PropagatedPanic from poisoned memo, got: {:?}",
        err
    );
}

/// Test that local cancellation via CancellationToken does NOT poison
/// cycle queries, because local cancellation is properly disabled during
/// cycle iteration.
#[test]
fn local_cancellation_does_not_poison_cycle_query() {
    let db = Knobs::default();

    // Create clones BEFORE setting up signal handlers
    let db_t1 = db.clone();

    // Create input before setting up signal handlers
    let input = Input::new(&db, 42);

    // Get cancellation token for t1
    let token = db_t1.cancellation_token();

    // Set up: when thread 1 signals stage 1, we'll cancel it
    // But this should NOT affect the cycle query because local cancellation
    // is disabled during execute_maybe_iterate

    // Thread 1: Start a cycle query
    let t1 = thread::spawn({
        let db = db_t1;
        move || {
            // This query has cycle recovery, so cancellation should be disabled
            cycle_query(&db, input)
        }
    });

    // Wait for thread 1 to enter the cycle query
    db.wait_for(1);

    // Try to cancel - but this should be ignored because cancellation is disabled
    token.cancel();

    // Let thread 1 continue
    db.signal(2);

    // Thread 1 should complete successfully despite cancellation request
    // because local cancellation is disabled during cycle iteration
    let r1 = t1.join().unwrap();
    assert_eq!(
        r1, 42,
        "Query should complete successfully despite cancellation request"
    );
}

/// Test that after a revision bump, the poisoned memo is cleared and
/// the query can succeed again.
#[test]
fn revision_bump_clears_poisoned_memo() {
    let db = Knobs::default();

    // Create clones BEFORE setting up signal handlers
    let db_writer = db.clone();
    let db_t1 = db.clone();
    let db_waiter = db.clone();

    // Create input before setting up signal handlers
    let input = Input::new(&db, 42);

    // Set up: when cancellation flag is set, signal stage 2 so thread 1 can continue
    db.signal_on_did_cancel(2);

    // Drop the original db so trigger_lru_eviction can complete
    drop(db);

    // Thread 1: Start a cycle query that will be interrupted
    let t1 = thread::spawn(move || catch_unwind(|| cycle_query(&db_t1, input)));

    // Wait for thread 1 to enter the cycle query
    db_waiter.wait_for(1);

    // Drop waiter so trigger_lru_eviction can proceed
    drop(db_waiter);

    // Thread 2: Trigger LRU eviction
    let t2 = thread::spawn({
        let mut db_writer = db_writer;
        move || {
            db_writer.trigger_lru_eviction();
            db_writer
        }
    });

    // Thread 1 should have been cancelled
    let r1 = t1.join().unwrap();
    assert!(r1.is_err());

    // Thread 2 should complete
    let db_after = t2.join().unwrap();

    // Thread 1: Start a cycle query that will be interrupted
    let t3 = thread::spawn(move || (catch_unwind(|| cycle_query(&db_after, input)), db_after));

    // Thread 3 should still fail as the poisoning has not been cleared from a revision bump so far.
    let (r3, mut db_after) = t3.join().unwrap();
    assert!(r3.is_err());

    // Trigger a synthetic write to bump the revision
    db_after.synthetic_write(salsa::Durability::HIGH);

    // NOW the query should succeed because the revision was bumped
    // and the poisoned memo is no longer valid
    let result = catch_unwind(|| cycle_query(&db_after, input));
    assert!(
        result.is_ok(),
        "Query should succeed after revision bump, got: {:?}",
        result.unwrap_err()
    );
    assert_eq!(result.unwrap(), 42);
}
