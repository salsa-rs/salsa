#![allow(clippy::needless_borrow)]

use derive_new::new;
use ordered_float::OrderedFloat;

// ANCHOR: input
#[salsa::input]
pub struct SourceProgram {
    #[return_ref]
    pub text: String,
}
// ANCHOR_END: input

// ANCHOR: interned_ids
#[salsa::interned]
pub struct VariableId {
    #[return_ref]
    pub text: String,
}

#[salsa::interned]
pub struct FunctionId {
    #[return_ref]
    pub text: String,
}
// ANCHOR_END: interned_ids

// ANCHOR: program
#[salsa::tracked]
pub struct Program {
    #[return_ref]
    pub statements: Vec<Statement>,
}
// ANCHOR_END: program

// ANCHOR: statements_and_expressions
#[derive(Eq, PartialEq, Debug, Hash, new)]
pub struct Statement {
    pub span: Span,

    pub data: StatementData,
}

#[derive(Eq, PartialEq, Debug, Hash)]
pub enum StatementData {
    /// Defines `fn <name>(<args>) = <body>`
    Function(Function),
    /// Defines `print <expr>`
    Print(Expression),
}

#[derive(Eq, PartialEq, Debug, Hash, new)]
pub struct Expression {
    pub span: Span,

    pub data: ExpressionData,
}

#[derive(Eq, PartialEq, Debug, Hash)]
pub enum ExpressionData {
    Op(Box<Expression>, Op, Box<Expression>),
    Number(OrderedFloat<f64>),
    Variable(VariableId),
    Call(FunctionId, Vec<Expression>),
}

#[derive(Eq, PartialEq, Copy, Clone, Hash, Debug)]
pub enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
}
// ANCHOR_END: statements_and_expressions

// ANCHOR: functions
#[salsa::tracked]
pub struct Function {
    #[id]
    pub name: FunctionId,

    name_span: Span,

    #[return_ref]
    pub args: Vec<VariableId>,

    #[return_ref]
    pub body: Expression,
}
// ANCHOR_END: functions

#[salsa::tracked]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

// ANCHOR: diagnostic
#[salsa::accumulator]
pub struct Diagnostics(Diagnostic);

#[derive(new, Clone, Debug)]
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub message: String,
}
// ANCHOR_END: diagnostic
