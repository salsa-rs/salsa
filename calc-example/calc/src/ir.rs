#![allow(clippy::needless_borrow)]

use derive_new::new;
use ordered_float::OrderedFloat;

// ANCHOR: input
/// Represents the source program, which is the main input of our compiler.
/// Each program begins as a simple string which we will parse into an AST
/// and then evaluate to create the final result.
#[salsa::input]
pub struct SourceProgram {
    /// The source of the program.
    ///
    /// The `return_ref` annotation makes the `text(db)` getter
    /// return an `&String` that refers directly into the database
    /// rather than returning a clone of the `String`. It is often
    /// used for types, like `String`, that are expensive to clone.
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

    /// The absolute position of the start of this function.
    /// All spans within the function (including `name_span`)
    /// are stored relative to this location.
    anchor_location: Location,

    name_span: Span,

    #[return_ref]
    pub args: Vec<VariableId>,

    #[return_ref]
    pub body: Expression,
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

// ANCHOR: anchor
/// Anchors mark significant points in the program text,
/// such as the start of the input or the start of a function.
/// All other spans are relative to an anchor, ensuring that
/// the spans don't change even when other content is added before
/// the anchor point.
pub trait Anchor {
    fn anchor_location(&self, db: &dyn crate::Db) -> Location;
}
// ANCHOR_END: anchor

// ANCHOR: anchor_impls
impl Anchor for Program {
    fn anchor_location(&self, _db: &dyn crate::Db) -> Location {
        Location::start()
    }
}

impl Anchor for Function {
    fn anchor_location(&self, db: &dyn crate::Db) -> Location {
        Function::anchor_location(*self, db)
    }
}
// ANCHOR_END: anchor_impls

/// Represents a specific location into the source string
/// as a utf-8 offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location(usize);

impl Location {
    pub fn as_usize(self) -> usize {
        self.0
    }

    pub fn start() -> Self {
        Self(0)
    }
}

impl std::ops::Add<Offset> for Location {
    type Output = Location;

    fn add(self, rhs: Offset) -> Self::Output {
        Location(self.0 + rhs.0)
    }
}

impl std::ops::Add<usize> for Location {
    type Output = Location;

    fn add(self, rhs: usize) -> Self::Output {
        Location(self.0 + rhs)
    }
}

impl std::ops::AddAssign<usize> for Location {
    fn add_assign(&mut self, rhs: usize) {
        *self = *self + rhs
    }
}

impl std::ops::Sub<Location> for Location {
    type Output = Offset;

    fn sub(self, rhs: Location) -> Self::Output {
        Offset(self.0 - rhs.0)
    }
}

/// Represents an offset in the source program relative to some anchor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Offset(usize);

impl Offset {
    pub fn location(self, anchor: Location) -> Location {
        Location(self.0 + anchor.0)
    }
}

// ANCHOR: span
/// Stores the location of a piece of IR within the source text.
/// Spans are not stored as absolute values but rather relative to some enclosing anchor
/// (some struct that implements the `Anchor` trait).
/// This way, although the location of the anchor may change, the spans themselves rarely do.
/// So long as a function doesn't convert the span into its absolute form,
/// and thus read the anchor's precise location, it won't need to re-execute, even if the anchor
/// has moved about in the file.
///
/// **NB:** It is your job, when converting the span into relative positions,
/// to supply the correct anchor! For example, the anchor for the expressions
/// within a function body is the function itself.
#[derive(new, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    /// Start of the span, relative to the anchor.
    pub start: Offset,

    /// End of the span, relative to the anchor.
    pub end: Offset,
}
// ANCHOR_END: span

// ANCHOR: span_methods
impl Span {
    /// Returns the absolute (start, end) points of this span, relative to the given anchor.
    pub fn absolute_locations(
        &self,
        db: &dyn crate::Db,
        anchor: &dyn Anchor,
    ) -> (Location, Location) {
        let base = anchor.anchor_location(db);
        (base + self.start, base + self.end)
    }

    /// Compute the absolute start of the span, relative to the given anchor.
    pub fn start(&self, db: &dyn crate::Db, anchor: &dyn Anchor) -> Location {
        self.absolute_locations(db, anchor).0
    }

    /// Compute the absolute end of the span, relative to the given anchor.
    pub fn end(&self, db: &dyn crate::Db, anchor: &impl Anchor) -> Location {
        self.absolute_locations(db, anchor).1
    }
}
// ANCHOR_END: span_methods

// ANCHOR: diagnostic
#[salsa::accumulator]
pub struct Diagnostics(Diagnostic);

#[derive(new, Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: Location,
    pub end: Location,
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
