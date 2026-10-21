#![cfg(feature = "inventory")]

//! Regression test for losing dependencies when `cycle_initial` reuses a previous result.
//!
//! `value` first reads `payload` as a participant in `choose`'s cycle, then becomes a
//! cycle head and keeps that result through `cycle_initial`. Passing the previous
//! result must also preserve its dependencies.
//!
//! In the next revision, `choose` stops reading `value` but still returns `1`.
//! Without the retained `payload` dependency, backdating `choose` lets `value` reuse
//! the stale result `666` instead of recomputing to `0`, as a fresh database does.

use salsa::Setter;

#[salsa::input]
struct Input {
    #[returns(copy)]
    initial: u32,
    #[returns(copy)]
    payload: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = |db, _, _, input: Input| input.initial(db))]
fn choose(db: &dyn salsa::Database, input: Input) -> u32 {
    if choose(db, input) == 0 {
        value(db, input);
    }
    1
}

#[salsa::tracked(returns(copy), cycle_initial = initial_value)]
fn value(db: &dyn salsa::Database, input: Input) -> u32 {
    if choose(db, input) == 0 {
        input.payload(db)
    } else {
        value(db, input)
    }
}

fn initial_value(
    _db: &dyn salsa::Database,
    _id: salsa::Id,
    previous: Option<&u32>,
    _input: Input,
) -> u32 {
    previous.copied().unwrap_or(0)
}

#[test]
fn initial_value_retains_previous_result_dependencies() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 0, 666);

    // `value` first participates in `choose`'s cycle and reads `payload`.
    // It then becomes its own cycle head, retaining that participant result.
    assert_eq!(choose(&db, input), 1);
    assert_eq!(value(&db, input), 666);

    input.set_initial(&mut db).to(1);
    input.set_payload(&mut db).to(444);

    // `choose` drops its read of `value`, but still returns 1. Backdating it
    // must not hide changes to inputs used by `value`'s previous result.
    assert_eq!(choose(&db, input), 1);

    let fresh_db = salsa::DatabaseImpl::default();
    let fresh_input = Input::new(&fresh_db, 1, 444);
    assert_eq!(value(&fresh_db, fresh_input), 0);
    assert_eq!(value(&db, input), 0);
}
