#![cfg(feature = "inventory")]

//! An immediate fallback discards the previous participant value and its dependencies.

mod common;

use common::LogDatabase;
use salsa::Setter;

#[salsa::input]
struct Input {
    #[returns(copy)]
    initial: u32,
    #[returns(copy)]
    payload: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = |db, _, _, input: Input| input.initial(db))]
fn choose(db: &dyn LogDatabase, input: Input) -> u32 {
    if choose(db, input) == 0 {
        value(db, input);
    }
    1
}

#[salsa::tracked(returns(copy), cycle_result = |_, _, _: Input| 0)]
fn value(db: &dyn LogDatabase, input: Input) -> u32 {
    db.push_log("value".to_owned());
    if choose(db, input) == 0 {
        input.payload(db)
    } else {
        value(db, input)
    }
}

#[test]
fn fallback_discards_previous_result_dependencies() {
    let mut db = common::LoggerDatabase::default();
    let input = Input::new(&db, 0, 666);

    assert_eq!(choose(&db, input), 1);
    assert_eq!(value(&db, input), 0);
    db.clear_logs();

    input.set_initial(&mut db).to(1);
    input.set_payload(&mut db).to(444);

    // `choose` is backdated. The fallback must not retain the `payload` read
    // from the earlier participant value and unnecessarily execute `value`.
    assert_eq!(choose(&db, input), 1);
    assert_eq!(value(&db, input), 0);
    db.assert_logs_len(0);
}
