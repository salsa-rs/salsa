//! Delete cascade:
//!
//! * when we delete memoized data, also delete outputs from that data

use salsa::DebugWithDb;
use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    MyTracked,
    final_result,
    create_tracked_structs,
    contribution_from_struct,
    copy_field,
);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn final_result(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result({:?})", input));
    let mut sum = 0;
    for tracked_struct in create_tracked_structs(db, input) {
        sum += contribution_from_struct(db, tracked_struct);
    }
    sum
}

#[salsa::tracked]
struct MyTracked {
    field: u32,
}

#[salsa::tracked]
fn create_tracked_structs(db: &dyn Db, input: MyInput) -> Vec<MyTracked> {
    db.push_log(format!("intermediate_result({:?})", input));
    (0..input.field(db))
        .map(|i| MyTracked::new(db, i))
        .collect()
}

#[salsa::tracked]
fn contribution_from_struct(db: &dyn Db, tracked: MyTracked) -> u32 {
    let m = MyTracked::new(db, tracked.field(db));
    copy_field(db, m) * 2
}

#[salsa::tracked]
fn copy_field(db: &dyn Db, tracked: MyTracked) -> u32 {
    tracked.field(db)
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        match event.kind {
            salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. } => {
                self.push_log(format!("salsa_event({:?})", event.kind.debug(self)));
            }
            _ => {}
        }
    }

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
fn basic() {
    let mut db = Database::default();

    // Creates 3 tracked structs
    let input = MyInput::new(&mut db, 3);
    assert_eq!(final_result(&db, input), 2 * 2 + 1 * 2 + 0 * 2);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "intermediate_result(MyInput(Id { value: 1 }))",
        ]"#]]);

    // Creates only 2 tracked structs in this revision, should delete 1
    //
    // Expect to see 6 DidDiscard events. Three from the primary struct:
    //
    // * the struct itself
    // * the struct's field
    // * the `contribution_from_struct` result
    //
    // and then 3 more from the struct created by `contribution_from_struct`:
    //
    // * the struct itself
    // * the struct's field
    // * the `copy_field` result

    input.set_field(&mut db).to(2);
    assert_eq!(final_result(&db, input), 1 * 2 + 0 * 2);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput(Id { value: 1 }))",
            "salsa_event(WillDiscardStaleOutput { execute_key: DependencyIndex { ingredient_index: IngredientIndex(5), key_index: Some(Id { value: 1 }) }, output_key: DependencyIndex { ingredient_index: IngredientIndex(3), key_index: Some(Id { value: 3 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(3), key_index: Some(Id { value: 3 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(2), key_index: Some(Id { value: 3 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(6), key_index: Some(Id { value: 3 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(3), key_index: Some(Id { value: 6 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(2), key_index: Some(Id { value: 6 }) } })",
            "salsa_event(DidDiscard { key: DependencyIndex { ingredient_index: IngredientIndex(7), key_index: Some(Id { value: 6 }) } })",
            "final_result(MyInput(Id { value: 1 }))",
        ]"#]]);
}
