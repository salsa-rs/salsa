use crate::{
    ir::{Program, SourceProgram},
    parser::parse_statements,
    type_check::type_check_program,
};

#[salsa::tracked]
pub fn compile(db: &dyn crate::Db, source_program: SourceProgram) -> Program {
    let program = parse_statements(db, source_program);
    type_check_program(db, program);
    program
}
