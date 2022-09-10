use ir::{Diagnostics, SourceProgram};

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
    crate::ir::Span,
    crate::parser::parse_statements,
    crate::type_check::type_check_program,
    crate::type_check::type_check_function,
    crate::type_check::find_function,
);
// ANCHOR_END: jar_struct

// ANCHOR: jar_db
pub trait Db: salsa::DbWithJar<Jar> {}
// ANCHOR_END: jar_db

// ANCHOR: jar_db_impl
impl<DB> Db for DB where DB: ?Sized + salsa::DbWithJar<Jar> {}
// ANCHOR_END: jar_db_impl

mod compile;
mod db;
mod ir;
mod parser;
mod type_check;

pub fn main() {
    let db = db::Database::default();
    let source_program = SourceProgram::new(&db, String::new());
    compile::compile(&db, source_program);
    let diagnostics = compile::compile::accumulated::<Diagnostics>(&db, source_program);
    eprintln!("{diagnostics:?}");
}
