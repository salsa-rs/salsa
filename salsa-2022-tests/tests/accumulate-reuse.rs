//! Accumulator re-use test.
//!
//! Tests behavior when a query's only inputs
//! are the accumulated values from another query.

use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(List, Integers, compute);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input]
struct List {
    value: u32,
    next: Option<List>,
}

#[salsa::accumulator]
struct Integers(u32);

#[salsa::tracked]
fn compute(db: &dyn Db, input: List) -> u32 {
    db.push_log(format!("compute({:?})", input,));

    // always pushes 0
    Integers::push(db, 0);

    let result = if let Some(next) = input.next(db) {
        let next_integers = compute::accumulated::<Integers>(db, next);
        let v = input.value(db) + next_integers.iter().sum::<u32>();
        v
    } else {
        input.value(db)
    };

    // return value changes
    result
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_event(&self, _event: salsa::Event) {}
}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn test1() {
    let mut db = Database::default();

    let l1 = List::new(&db, 1, None);
    let l2 = List::new(&db, 2, Some(l1));

    assert_eq!(compute(&db, l2), 2);
    db.assert_logs(expect![[r#"
        [
            "compute(List(Id { value: 2 }))",
            "compute(List(Id { value: 1 }))",
        ]"#]]);

    // When we mutate `l1`, we should re-execute `compute` for `l1`,
    // but we should not have to re-execute `compute` for `l2`.
    // The only inpout for `compute(l1)` is the accumulated values from `l1`,
    // which have not changed.
    l1.set_value(&mut db).to(2);
    assert_eq!(compute(&db, l2), 2);
    db.assert_logs(expect![[r#"
        [
            "compute(List(Id { value: 2 }))",
            "compute(List(Id { value: 1 }))",
        ]"#]]);
}
