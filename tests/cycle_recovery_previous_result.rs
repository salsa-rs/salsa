#![cfg(feature = "inventory")]

//! Regression test for <https://github.com/salsa-rs/salsa/issues/1337>.
//!
//! Cycle recovery can retain a previous iteration's result even when the current
//! body no longer reads its inputs. Passing that result to recovery must record
//! its dependencies, just as an ordinary query read would.
//!
//! Here, an iteration of `a` creates a literal from `Input::payload`. A later
//! iteration returns `None`, but recovery retains the previous literal. Retroactive
//! cycle-head discovery through `b` makes `a` a cycle head without carrying over
//! the earlier payload dependency through ordinary dependency flattening.
//!
//! Changing `payload` must invalidate the retained result. On recomputation,
//! `choose` still returns `1`, and the new revision's cycle starts from `None`, so
//! `a` returns `None`. Without the fix, Salsa loses the payload dependency, and
//! reading the old literal panics because it was not revalidated for the new revision.

use salsa::Setter;

#[salsa::input]
struct Input {
    #[returns(copy)]
    phase: u32,
    #[returns(copy)]
    payload: u32,
}

#[salsa::interned]
struct Literal<'db> {
    #[returns(copy)]
    value: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn u(db: &dyn salsa::Database, input: Input) -> u32 {
    let w = w(db, input);
    if w == 1 {
        c(db, input);
    }
    input.phase(db)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn w(db: &dyn salsa::Database, input: Input) -> u32 {
    u(db, input).max(1)
}

#[salsa::tracked(cycle_initial = |_, _, _| ())]
fn c(db: &dyn salsa::Database, input: Input) {
    if input.phase(db) == 3 {
        // b finishes before c discovers its dependency on the outer a.
        b(db, input);
        a(db, input);
    }
}

#[salsa::tracked(cycle_initial = |_, _, _| ())]
fn b(db: &dyn salsa::Database, input: Input) {
    c(db, input);
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| None, cycle_fn = recover)]
fn a(db: &dyn salsa::Database, input: Input) -> Option<Literal<'_>> {
    if choose(db, input) == 0 {
        a(db, input);
        Some(Literal::new(db, input.payload(db)))
    } else {
        // w's old dependencies validate c, but its fresh execution omits c.
        // b then adds a as a cycle head without carrying a's previous inputs.
        w(db, input);
        b(db, input);
        None
    }
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 99)]
fn choose(db: &dyn salsa::Database, input: Input) -> u32 {
    if choose(db, input) == 99 {
        a(db, input);
        0
    } else {
        1
    }
}

fn recover<'db>(
    _db: &'db dyn salsa::Database,
    _cycle: &salsa::Cycle,
    previous: &Option<Literal<'db>>,
    current: Option<Literal<'db>>,
    _input: Input,
) -> Option<Literal<'db>> {
    current.or(*previous)
}

#[test]
fn recovery_retains_inputs_with_retroactive_cycle_heads() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 1, 666);

    // Warm from u, then enter the old cycle from w through a.
    u(&db, input);
    input.set_phase(&mut db).to(3);
    assert!(a(&db, input).is_some());

    // Recovery retained the old literal; its dependency must be revalidated.
    input.set_payload(&mut db).to(444);
    let result = a(&db, input).map(|literal| literal.value(&db));
    assert_eq!(result, None);
}
