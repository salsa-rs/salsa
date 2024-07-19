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
#[derive(Eq, PartialEq, Debug, Hash, new, salsa::Update)]
pub struct Statement<'db> {
    pub span: Span<'db>,

    pub data: StatementData<'db>,
}

#[derive(Eq, PartialEq, Debug, Hash, salsa::Update)]
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

#[derive(Eq, PartialEq, Debug, Hash, salsa::Update)]
pub enum ExpressionData<'db> {
    Op(Box<Expression<'db>>, Op, Box<Expression<'db>>),
    Number(OrderedFloat<f64>),
    Variable(VariableId<'db>),
    Call(FunctionId<'db>, Vec<Expression<'db>>),
}

#[derive(Eq, PartialEq, Copy, Clone, Hash, Debug, salsa::Update)]
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
#[allow(dead_code)] // Debug impl uses them
#[derive(new)]
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub message: String,
}
// ANCHOR_END: diagnostic

impl Diagnostic {
    #[cfg(test)]
    pub fn render(&self, db: &dyn crate::Db, src: SourceProgram) -> String {
        use annotate_snippets::*;
        let line_start = src.text(db)[..self.start].lines().count() + 1;
        Renderer::plain()
            .render(
                Level::Error.title(&self.message).snippet(
                    Snippet::source(src.text(db))
                        .line_start(line_start)
                        .origin("input")
                        .annotation(Level::Error.span(self.start..self.end).label("here")),
                ),
            )
            .to_string()
    }
}
