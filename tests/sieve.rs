#![cfg(feature = "inventory")]

//! Tests for the SIEVE eviction policy.

mod common;
use common::LogDatabase;

use salsa::Database as _;
use test_log::test;

#[salsa::input]
struct MyInput {
    #[returns(copy)]
    field: u32,
}

#[salsa::tracked(returns(copy), sieve = 2)]
fn sieve_value(db: &dyn LogDatabase, input: MyInput) -> u32 {
    db.push_log(format!("sieve_value({:?})", input.field(db)));
    input.field(db)
}

#[test]
fn sieve_gives_visited_values_a_second_chance() {
    let mut db = common::LoggerDatabase::default();

    let inputs: Vec<MyInput> = (0..3).map(|field| MyInput::new(&db, field)).collect();

    assert_eq!(sieve_value(&db, inputs[0]), 0);
    assert_eq!(sieve_value(&db, inputs[1]), 1);

    // Touch `0` again after admission. SIEVE should set its visited bit without
    // moving it out of its old FIFO position.
    assert_eq!(sieve_value(&db, inputs[0]), 0);

    assert_eq!(sieve_value(&db, inputs[2]), 2);
    db.assert_logs_len(3);

    db.synthetic_write(salsa::Durability::HIGH);

    assert_eq!(sieve_value(&db, inputs[0]), 0);
    db.assert_logs_len(0);

    assert_eq!(sieve_value(&db, inputs[1]), 1);
    db.assert_logs(expect_test::expect![[r#"
        [
            "sieve_value(1)",
        ]"#]]);
}

#[test]
fn sieve_readmits_evicted_values() {
    let mut db = common::LoggerDatabase::default();

    let inputs: Vec<MyInput> = (0..3).map(|field| MyInput::new(&db, field)).collect();

    assert_eq!(sieve_value(&db, inputs[0]), 0);
    assert_eq!(sieve_value(&db, inputs[1]), 1);
    assert_eq!(sieve_value(&db, inputs[2]), 2);
    db.assert_logs_len(3);

    db.synthetic_write(salsa::Durability::HIGH);

    assert_eq!(sieve_value(&db, inputs[0]), 0);
    db.assert_logs(expect_test::expect![[r#"
        [
            "sieve_value(0)",
        ]"#]]);

    db.synthetic_write(salsa::Durability::HIGH);

    assert_eq!(sieve_value(&db, inputs[0]), 0);
    db.assert_logs_len(0);

    assert_eq!(sieve_value(&db, inputs[1]), 1);
    db.assert_logs(expect_test::expect![[r#"
        [
            "sieve_value(1)",
        ]"#]]);
}
