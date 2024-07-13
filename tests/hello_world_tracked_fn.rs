//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::tracked]
fn final_result(dbx: &dyn Db, a: i32, (b, c): (i32, i32)) -> i32 {
    dbx.push_log(format!("final_result({a}, {b}, {c})"));
    a + b * c
}

// salsa::plumbing::setup_interned_fn!(
//     vis: ,
//     fn_name: identity,
//     db_lt: 'db,
//     Db: Db,
//     db: dbx,
//     input_ids: [input1, input2],
//     input_tys: [i32, (i32, i32)],
//     output_ty: i32,
//     inner_fn: fn inner1(dbx: &dyn Db, a: i32, (b, c): (i32, i32)) -> i32 {
//         dbx.push_log(format!("final_result({a}, {b}, {c})"));
//         a + b * c
//     },
//     cycle_recovery_fn: (salsa::plumbing::unexpected_cycle_recovery!),
//     cycle_recovery_strategy: Panic,
//     unused_names: [
//         zalsa1,
//         Configuration1,
//         InternedData1,
//         FN_CACHE1,
//         INTERN_CACHE1,
//         inner1,
//     ]
// );
