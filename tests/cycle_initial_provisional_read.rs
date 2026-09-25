#![cfg(feature = "inventory")]

//! Initializers can depend on provisional results from an already established cycle.
//!
//! This pattern is strongly discouraged, but occurs in real-world code, including ty's annotation
//! inference. This test preserves support for that existing usage.
//!
//! `outer` calls itself first, installing an initial value read from `input.value`, then calls
//! `inner`. `inner` also calls itself; its initializer reads the provisional value of `outer`.
//! This records a dependency inside an initializer while the outer query is still running.
//!
//! That dependency must be flattened to `outer`'s underlying inputs. Otherwise, later cycle
//! completion copies the unflattened dependency on `outer` back into `outer`'s own dependencies.
//! Validation in the next revision then encounters a cycle and reexecutes both query bodies.
//!
//! A synthetic write advances the revision without changing the input. With flattened
//! dependencies, validation can reuse both results and the log stays empty. Without flattening,
//! the returned value is still `42`, but the log contains `outer` and `inner`, failing the assertion.

use salsa::{Database, Durability};

use crate::common::LogDatabase;

mod common;

#[salsa::input]
struct Input {
    #[returns(copy)]
    value: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = |db, _, input: Input| input.value(db))]
fn outer(db: &dyn LogDatabase, input: Input) -> u32 {
    db.push_log("outer".to_owned());
    // Establish a provisional value before entering the inner cycle.
    outer(db, input);
    inner(db, input)
}

#[salsa::tracked(returns(copy), cycle_initial = |db, _, input| outer(db, input))]
fn inner(db: &dyn LogDatabase, input: Input) -> u32 {
    db.push_log("inner".to_owned());
    inner(db, input)
}

#[test]
fn initializer_can_read_an_existing_provisional_value() {
    let mut db = common::LoggerDatabase::default();
    let input = Input::new(&db, 42);
    assert_eq!(outer(&db, input), 42);

    db.clear_logs();
    db.synthetic_write(Durability::LOW);
    assert_eq!(outer(&db, input), 42);
    db.assert_logs(expect_test::expect![[r"[]"]]);
}
