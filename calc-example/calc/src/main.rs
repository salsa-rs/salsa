use ir::SourceProgram;

// ANCHOR: jar_struct
#[salsa::jar(db = Db)]
pub struct Jar(
    crate::compile::compile,
    crate::ir::SourceProgram,
    crate::ir::Program,
    crate::ir::VariableId,
    crate::ir::FunctionId,
    crate::ir::Function,
    crate::ir::Diagnostics,
    crate::parser::parse_source_program,
    crate::type_check::type_check_program,
    crate::type_check::type_check_function,
    crate::evaluate::evaluate_function,
    crate::ir::find_function,
);
// ANCHOR_END: jar_struct

// ANCHOR: jar_db
pub trait Db: salsa::DbWithJar<Jar> + PushLog {}
// ANCHOR_END: jar_db

// ANCHOR: jar_db_impl
impl<DB> Db for DB where DB: ?Sized + PushLog + salsa::DbWithJar<Jar> {}
// ANCHOR_END: jar_db_impl

// ANCHOR: PushLog
pub trait PushLog {
    /// When testing, invokes `message` to create a log string and
    /// pushes that string onto an internal list of logs.
    ///
    /// This list of logs can later be used to observe what got re-executed
    /// or modified during execution.
    fn push_log(&self, message: &mut dyn FnMut() -> String);
}
// ANCHOR_END: PushLog

mod compile;
mod db;
mod evaluate;
mod ir;
mod parser;
mod type_check;

pub fn main() {
    let mut db = db::Database::default();
    let source_program = SourceProgram::new(&mut db, String::new());
    match evaluate::evaluate_source_program(&db, source_program) {
        Ok(s) => println!("{s}"),
        Err(d) => eprintln!("{d:#?}"), // FIXME attach ariadne crate or something
    }
}
