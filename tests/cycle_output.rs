#![cfg(feature = "inventory")]

//! Test tracked struct output from a query in a cycle.
mod common;
use common::{HasLogger, LogDatabase, Logger};
use expect_test::expect;
use salsa::{Setter, Storage};

#[salsa::tracked]
struct Output<'db> {
    value: u32,
}

#[salsa::input]
struct InputValue {
    value: u32,
}

#[salsa::tracked]
fn read_value<'db>(db: &'db dyn Db, output: Output<'db>) -> u32 {
    output.value(db)
}

#[salsa::tracked]
fn query_a(db: &dyn Db, input: InputValue) -> u32 {
    let val = query_b(db, input);
    let output = Output::new(db, val);
    let read = read_value(db, output);
    assert_eq!(read, val);
    query_d(db);
    if val > 2 {
        val
    } else {
        val + input.value(db)
    }
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query_b(db: &dyn Db, input: InputValue) -> u32 {
    query_a(db, input)
}

fn cycle_initial(_db: &dyn Db, _input: InputValue) -> u32 {
    0
}

fn cycle_fn(
    _db: &dyn Db,
    _value: &u32,
    _count: u32,
    _input: InputValue,
) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Iterate
}

#[salsa::tracked]
fn query_c(db: &dyn Db, input: InputValue) -> u32 {
    input.value(db)
}

#[salsa::tracked]
fn query_d(db: &dyn Db) -> u32 {
    db.get_input().map(|input| input.value(db)).unwrap_or(0)
}

trait HasOptionInput {
    fn get_input(&self) -> Option<InputValue>;
    fn set_input(&mut self, input: InputValue);
}

#[salsa::db]
trait Db: HasOptionInput + salsa::Database {}

#[salsa::db]
#[derive(Clone)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
    input: Option<InputValue>,
}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

impl Default for Database {
    fn default() -> Self {
        let logger = Logger::default();
        Self {
            storage: Storage::new(Some(Box::new({
                let logger = logger.clone();
                move |event| match event.kind {
                    salsa::EventKind::WillExecute { .. }
                    | salsa::EventKind::DidValidateMemoizedValue { .. } => {
                        logger.push_log(format!("salsa_event({:?})", event.kind));
                    }
                    salsa::EventKind::WillCheckCancellation => {}
                    _ => {
                        logger.push_log(format!("salsa_event({:?})", event.kind));
                    }
                }
            }))),
            logger,
            input: Default::default(),
        }
    }
}

impl HasOptionInput for Database {
    fn get_input(&self) -> Option<InputValue> {
        self.input
    }

    fn set_input(&mut self, input: InputValue) {
        self.input.replace(input);
    }
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

#[test_log::test]
fn single_revision() {
    let db = Database::default();
    let input = InputValue::new(&db, 1);

    assert_eq!(query_b(&db, input), 3);
}

#[test_log::test]
fn revalidate_no_changes() {
    let mut db = Database::default();

    let ab_input = InputValue::new(&db, 1);
    let c_input = InputValue::new(&db, 10);
    assert_eq!(query_c(&db, c_input), 10);
    assert_eq!(query_b(&db, ab_input), 3);

    db.assert_logs_len(15);

    // trigger a new revision, but one that doesn't touch the query_a/query_b cycle
    c_input.set_value(&mut db).to(20);

    assert_eq!(query_b(&db, ab_input), 3);

    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidSetCancellationFlag)",
            "salsa_event(DidValidateMemoizedValue { database_key: read_value(Id(400)) })",
            "salsa_event(DidValidateInternedValue { key: query_d::interned_arguments(Id(800)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_d(Id(800)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: read_value(Id(401)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: read_value(Id(402)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: read_value(Id(403)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_b(Id(0)) })",
        ]"#]]);
}

#[test_log::test]
fn revalidate_with_change_after_output_read() {
    let mut db = Database::default();

    let ab_input = InputValue::new(&db, 1);
    let d_input = InputValue::new(&db, 10);
    db.set_input(d_input);

    assert_eq!(query_b(&db, ab_input), 3);

    db.assert_logs_len(14);

    // trigger a new revision that changes the output of query_d
    d_input.set_value(&mut db).to(20);

    assert_eq!(query_b(&db, ab_input), 3);

    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidSetCancellationFlag)",
            "salsa_event(DidValidateMemoizedValue { database_key: read_value(Id(400)) })",
            "salsa_event(DidValidateInternedValue { key: query_d::interned_arguments(Id(800)), revision: R2 })",
            "salsa_event(WillExecute { database_key: query_b(Id(0)) })",
            "salsa_event(WillExecute { database_key: query_a(Id(0)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(800)) })",
            "salsa_event(WillDiscardStaleOutput { execute_key: query_a(Id(0)), output_key: Output(Id(401)) })",
            "salsa_event(DidDiscard { key: Output(Id(401)) })",
            "salsa_event(DidDiscard { key: read_value(Id(401)) })",
            "salsa_event(WillDiscardStaleOutput { execute_key: query_a(Id(0)), output_key: Output(Id(402)) })",
            "salsa_event(DidDiscard { key: Output(Id(402)) })",
            "salsa_event(DidDiscard { key: read_value(Id(402)) })",
            "salsa_event(WillDiscardStaleOutput { execute_key: query_a(Id(0)), output_key: Output(Id(403)) })",
            "salsa_event(DidDiscard { key: Output(Id(403)) })",
            "salsa_event(DidDiscard { key: read_value(Id(403)) })",
            "salsa_event(WillIterateCycle { database_key: query_b(Id(0)), iteration_count: IterationCount(1) })",
            "salsa_event(WillExecute { database_key: query_a(Id(0)) })",
            "salsa_event(WillExecute { database_key: read_value(Id(401g1)) })",
            "salsa_event(WillIterateCycle { database_key: query_b(Id(0)), iteration_count: IterationCount(2) })",
            "salsa_event(WillExecute { database_key: query_a(Id(0)) })",
            "salsa_event(WillExecute { database_key: read_value(Id(402g1)) })",
            "salsa_event(WillIterateCycle { database_key: query_b(Id(0)), iteration_count: IterationCount(3) })",
            "salsa_event(WillExecute { database_key: query_a(Id(0)) })",
            "salsa_event(WillExecute { database_key: read_value(Id(403g1)) })",
        ]"#]]);
}
