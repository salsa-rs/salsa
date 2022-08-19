use ordered_float::OrderedFloat;
use salsa::debug::DebugWithDb;

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

// ANCHOR: statements_and_expressions
#[salsa::tracked]
pub struct Program {
    statements: Vec<Statement>,
}

#[salsa::interned]
pub struct Statement {
    data: StatementData,
}

#[derive(Eq, PartialEq, Clone, Hash)]
pub enum StatementData {
    /// Defines `fn <name>(<args>) = <body>`
    Function(Function),
    /// Defines `print <expr>`
    Print(Expression),
}

#[salsa::interned]
pub struct Expression {
    #[return_ref]
    data: ExpressionData,
}

#[derive(Eq, PartialEq, Clone, Hash)]
pub enum ExpressionData {
    Op(Expression, Op, Expression),
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

impl DebugWithDb<dyn crate::Db + '_> for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        f.debug_struct("Function")
            .field("name", &self.name(db).debug(db))
            .field("args", &self.args(db).debug(db))
            .field("body", &self.body(db).debug(db))
            .finish()
    }
}

impl DebugWithDb<dyn crate::Db + '_> for Statement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        match self.data(db) {
            StatementData::Function(a) => DebugWithDb::fmt(&a, f, db),
            StatementData::Print(a) => DebugWithDb::fmt(&a, f, db),
        }
    }
}

// ANCHOR: expression_debug_impl
impl DebugWithDb<dyn crate::Db + '_> for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        match self.data(db) {
            ExpressionData::Op(a, b, c) => f
                .debug_tuple("ExpressionData::Op")
                .field(&a.debug(db)) // use `a.debug(db)` for interned things
                .field(&b.debug(db))
                .field(&c.debug(db))
                .finish(),
            ExpressionData::Number(a) => {
                f.debug_tuple("Number")
                    .field(a) // use just `a` otherwise
                    .finish()
            }
            ExpressionData::Variable(a) => f.debug_tuple("Variable").field(&a.debug(db)).finish(),
            ExpressionData::Call(a, b) => f
                .debug_tuple("Call")
                .field(&a.debug(db))
                .field(&b.debug(db))
                .finish(),
        }
    }
}
// ANCHOR_END: expression_debug_impl

impl DebugWithDb<dyn crate::Db + '_> for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        f.debug_struct("Program")
            .field("statements", &self.statements(db))
            .finish()
    }
}

impl DebugWithDb<dyn crate::Db + '_> for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        write!(f, "{:?}", self.text(db))
    }
}

impl DebugWithDb<dyn crate::Db + '_> for VariableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        write!(f, "{:?}", self.text(db))
    }
}

// ANCHOR: op_debug_impl
impl DebugWithDb<dyn crate::Db + '_> for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, _db: &dyn crate::Db) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
// ANCHOR: op_debug_impl

impl DebugWithDb<dyn crate::Db + '_> for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, _db: &dyn crate::Db) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ANCHOR: functions
#[salsa::tracked]
pub struct Function {
    #[id]
    name: FunctionId,
    args: Vec<VariableId>,
    body: Expression,
}
// ANCHOR_END: functions

// ANCHOR: diagnostic
#[salsa::accumulator]
pub struct Diagnostics(Diagnostic);

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub position: usize,
    pub message: String,
}
// ANCHOR_END: diagnostic
