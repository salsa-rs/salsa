use expect_test::expect;
use salsa::{Database as SalsaDatabase, DebugWithDb};
mod common;
use common::{HasLogger, Logger};

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked<'_>, tracked_fn, tracked_fn_extra);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
fn tracked_fn<'db>(db: &'db dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("tracked_fn({:?})", input.debug(db)));
    let t = MyTracked::new(db, input.field(db) * 2);
    tracked_fn_extra::specify(db, t, 2222);
    tracked_fn_extra(db, t)
}

#[salsa::tracked(jar = Jar, specify)]
fn tracked_fn_extra<'db>(db: &dyn Db, input: MyTracked<'db>) -> u32 {
    db.push_log(format!("tracked_fn_extra({:?})", input.debug(db)));
    0
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn execute() {
    let mut db = Database::default();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input), 2222);
    db.assert_logs(expect![[r#"
        [
            "tracked_fn(MyInput { [salsa id]: 0, field: 22 })",
        ]"#]]);

    // A "synthetic write" causes the system to act *as though* some
    // input of durability `durability` has changed.
    db.synthetic_write(salsa::Durability::LOW);

    // Re-run the query on the original input. Nothing re-executes!
    assert_eq!(tracked_fn(&db, input), 2222);
    db.assert_logs(expect!["[]"]);
}
