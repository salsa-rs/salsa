mod common;

use common::{HasLogger, Logger};
use expect_test::expect;
use salsa::Setter as _;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::input]
struct Input {
    number: i16,
}

#[salsa::tracked(no_eq)]
fn abs_float(db: &dyn Db, input: Input) -> f32 {
    let number = input.number(db);

    db.push_log(format!("abs_float({number})"));
    number.abs() as f32
}

#[salsa::tracked]
fn derived(db: &dyn Db, input: Input) -> u32 {
    let x = abs_float(db, input);
    db.push_log("derived".to_string());

    x as u32
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

#[test]
fn invoke() {
    let mut db = Database::default();

    let input = Input::new(&db, 5);
    let x = derived(&db, input);

    assert_eq!(x, 5);

    input.set_number(&mut db).to(-5);

    // Derived should re-execute even the result of `abs_float` is the same.
    let x = derived(&db, input);
    assert_eq!(x, 5);

    db.assert_logs(expect![[r#"
        [
            "abs_float(5)",
            "derived",
            "abs_float(-5)",
            "derived",
        ]"#]]);
}
