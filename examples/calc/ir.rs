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
pub struct VariableId<'db> {
    #[return_ref]
    pub text: String,
}

#[salsa::interned]
pub struct FunctionId<'db> {
    #[return_ref]
    pub text: String,
}
// ANCHOR_END: interned_ids

// ANCHOR: program
#[salsa::tracked]
pub struct Program<'db> {
    #[return_ref]
    pub statements: Vec<Statement<'db>>,
}
// ANCHOR_END: program

// ANCHOR: statements_and_expressions
#[derive(Eq, PartialEq, Debug, Hash, new, salsa::Update, salsa::DebugWithDb)]
pub struct Statement<'db> {
    pub span: Span<'db>,

    pub data: StatementData<'db>,
}

#[derive(Eq, PartialEq, Debug, Hash, salsa::Update, salsa::DebugWithDb)]
pub enum StatementData<'db> {
    /// Defines `fn <name>(<args>) = <body>`
    Function(Function<'db>),
    /// Defines `print <expr>`
    Print(Expression<'db>),
}

#[derive(Eq, PartialEq, Debug, Hash, new, salsa::Update)]
pub struct Expression<'db> {
    pub span: Span<'db>,

    pub data: ExpressionData<'db>,
}

#[derive(Eq, PartialEq, Debug, Hash, salsa::Update, salsa::DebugWithDb)]
pub enum ExpressionData<'db> {
    Op(Box<Expression<'db>>, Op, Box<Expression<'db>>),
    Number(OrderedFloat<f64>),
    Variable(VariableId<'db>),
    Call(FunctionId<'db>, Vec<Expression<'db>>),
}

#[derive(Eq, PartialEq, Copy, Clone, Hash, Debug, salsa::Update, salsa::DebugWithDb)]
pub enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
}
// ANCHOR_END: statements_and_expressions

// ANCHOR: functions
#[salsa::tracked]
pub struct Function<'db> {
    #[id]
    pub name: FunctionId<'db>,

    name_span: Span<'db>,

    #[return_ref]
    pub args: Vec<VariableId<'db>>,

    #[return_ref]
    pub body: Expression<'db>,
}
// ANCHOR_END: functions

#[salsa::tracked]
pub struct Span<'db> {
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
