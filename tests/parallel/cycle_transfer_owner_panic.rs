// Shuttle doesn't support panics inside its runtime.
#![cfg(not(feature = "shuttle"))]

use salsa::Setter;

use crate::sync::thread;
use crate::{Knobs, KnobsDatabase};

#[salsa::input]
struct Input {
    #[returns(copy)]
    epoch: u32,
    #[returns(copy)]
    nested: bool,
}

#[derive(Clone, Debug, Eq)]
struct Value {
    epoch: u32,
    value: u32,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        // Fixpoint comparisons stay within one epoch. Comparing against the old
        // memo during backdating panics after the participant has finalized.
        if self.epoch != other.epoch {
            panic!("panic during backdating after convergence");
        }
        self.value == other.value
    }
}

#[salsa::tracked(returns(clone), cycle_initial = initial_owner)]
fn owner(db: &dyn KnobsDatabase, input: Input) -> Value {
    Value {
        epoch: input.epoch(db),
        value: participant(db, input),
    }
}

fn initial_owner(db: &dyn KnobsDatabase, _: salsa::Id, input: Input) -> Value {
    Value {
        epoch: input.epoch(db),
        value: 0,
    }
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn participant(db: &dyn KnobsDatabase, input: Input) -> u32 {
    if input.epoch(db) == 1 {
        db.signal(1);
        db.wait_for(2);
    }
    let mut value = owner(db, input).value;
    if input.nested(db) {
        value = value.max(inner(db, input));
    }
    (value + 1).min(3)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn inner(db: &dyn KnobsDatabase, input: Input) -> u32 {
    participant(db, input)
}

// Regression test for panic propagation when two threads evaluate mutually
// recursive queries. The test first caches their results, changes an input, then
// starts one thread evaluating `participant` and another evaluating `owner`.
// The cycle converges, but `owner` panics when comparing its new result with the
// cached result from the previous revision. The waiting participant thread must
// also report a panic, rather than successfully returning a value.
//
// Internally, `participant` transfers its claim to `owner` and waits for it.
// Previously, this wait discarded `WaitResult::Panicked` and retried the fetch,
// which returned the already-finalized participant memo. It must instead
// propagate `Cancelled::PropagatedPanic`.
#[test]
fn transfer_wait_propagates_panic_after_convergence() {
    check_transfer_wait_propagates_panic(false);
}

#[test]
fn nested_transfer_wait_propagates_panic_after_convergence() {
    // In addition to owner -> participant -> owner, form participant -> inner ->
    // participant so that a cycle head transfers through try_complete_cycle_head.
    check_transfer_wait_propagates_panic(true);
}

fn check_transfer_wait_propagates_panic(nested: bool) {
    let mut db = Knobs::default();
    let input = Input::new(&db, 0, nested);
    assert_eq!(owner(&db, input).value, 3);

    input.set_epoch(&mut db).to(1);
    let db_waiter = db.clone();
    let db_owner = db.clone();
    db_owner.signal_on_will_block(2);

    // Start the participant first, then make the owner block on it. The
    // participant closes the cycle and transfers its claim to the owner.
    let waiter = thread::spawn(move || participant(&db_waiter, input));
    db.wait_for(1);
    let owner = thread::spawn(move || owner(&db_owner, input));

    let owner_result = owner.join();
    let waiter_result = waiter.join();
    let owner_panic = owner_result.expect_err("the owner should panic while backdating");
    assert_eq!(
        owner_panic.downcast_ref::<&str>(),
        Some(&"panic during backdating after convergence")
    );
    let waiter_panic = waiter_result.expect_err("the transfer wait should propagate the panic");
    assert!(matches!(
        waiter_panic.downcast_ref::<salsa::Cancelled>(),
        Some(salsa::Cancelled::PropagatedPanic)
    ));
}
