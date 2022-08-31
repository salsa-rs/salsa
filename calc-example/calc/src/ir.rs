#![allow(clippy::needless_borrow)]

use derive_new::new;
use ordered_float::OrderedFloat;

// ANCHOR: input
#[salsa::input]
pub struct SourceProgram {
    #[return_ref]
    text: String,
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
    statements: Vec<Statement>,
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
    name: FunctionId,

    name_span: Span,

    #[return_ref]
    args: Vec<VariableId>,

    #[return_ref]
    body: Expression,
}
// ANCHOR_END: functions

#[salsa::tracked]
pub(crate) fn find_function(
    db: &dyn crate::Db,
    program: Program,
    name: FunctionId,
) -> Option<Function> {
    program
        .statements(db)
        .iter()
        .filter_map(|s| match s.data {
            StatementData::Function(func) if func.name(db) == name => Some(func),
            _ => None,
        })
        .next()
}

impl Program {
    pub(crate) fn find_function(self, db: &dyn crate::Db, name: FunctionId) -> Option<Function> {
        find_function(db, self, name)
    }
}

/// Anchors mark either the start of the input itself
/// *or* the start a function.
/// The spans used to identify the location of other bits of IR,
/// such as an expression,
/// are always relative to an anchor.
/// Making spans relative to an anchor means that they do not
/// change when the body of some prior function is modified.
#[salsa::tracked]
pub struct Anchor {
    location: usize,
}

/// Stores the location of a piece of IR within the source text.
#[derive(new, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    /// The other fields in the span are "relative" to the given anchor.
    pub anchor: Anchor,

    /// Start of the span, relative to the anchor.
    pub start: usize,

    /// End of the span, relative to the anchor.
    pub end: usize,
}

impl Span {
    /// Compute the absolute start of the span, relative to the start of the input.-
    pub fn start(&self, db: &dyn crate::Db) -> usize {
        self.anchor.location(db) + self.start
    }

    /// Compute the absolute end of the span, relative to the start of the input.-
    pub fn end(&self, db: &dyn crate::Db) -> usize {
        self.anchor.location(db) + self.start
    }
}

// ANCHOR: diagnostic
#[salsa::accumulator]
pub struct Diagnostics(Diagnostic);

#[derive(new, Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub message: String,
}
// ANCHOR_END: diagnostic

// ANCHOR: diagnostic_debug
impl<Db: ?Sized> salsa::DebugWithDb<Db> for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, _db: &Db) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
// ANCHOR_END: diagnostic_debug
