//! Accumulate values from within a tracked function.
//! Then mutate the values so that the tracked function re-executes.
//! Check that we accumulate the appropriate, new values.

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
fn compute(db: &dyn Db, input: List) {
    eprintln!(
        "{:?}(value={:?}, next={:?})",
        input,
        input.value(db),
        input.next(db)
    );
    let result = if let Some(next) = input.next(db) {
        let next_integers = compute::accumulated::<Integers>(db, next);
        eprintln!("{:?}", next_integers);
        let v = input.value(db) + next_integers.iter().sum::<u32>();
        eprintln!("input={:?} v={:?}", input.value(db), v);
        v
    } else {
        input.value(db)
    };
    Integers::push(db, result);
    eprintln!("pushed result {:?}", result);
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_event(&self, _event: salsa::Event) {}

    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
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

    let l0 = List::new(&mut db, 1, None);
    let l1 = List::new(&mut db, 10, Some(l0));

    compute(&db, l1);
    expect![[r#"
        [
            11,
            1,
        ]
    "#]]
    .assert_debug_eq(&compute::accumulated::<Integers>(&db, l1));

    l0.set_value(&mut db).to(2);
    compute(&db, l1);
    expect![[r#"
        [
            12,
            2,
        ]
    "#]]
    .assert_debug_eq(&compute::accumulated::<Integers>(&db, l1));
}
