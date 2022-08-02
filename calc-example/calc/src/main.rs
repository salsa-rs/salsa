// ANCHOR: jar_struct
#[salsa::jar(db = Db)]
pub struct Jar(
    crate::ir::VariableId,
    crate::ir::FunctionId,
    crate::ir::Expression,
    crate::ir::Statement,
    crate::ir::Function,
    crate::ir::Diagnostics,
    crate::parser::parse_statements,
    crate::parser::source_text,
);
// ANCHOR_END: jar_struct

// ANCHOR: jar_db
pub trait Db: salsa::DbWithJar<Jar> {}
// ANCHOR_END: jar_db

// ANCHOR: jar_db_impl
impl<DB> Db for DB where DB: ?Sized + salsa::DbWithJar<Jar> {}
// ANCHOR_END: jar_db_impl

mod db;
mod ir;
mod parser;

pub fn main() {}
