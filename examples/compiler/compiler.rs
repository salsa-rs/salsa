pub trait CompilerQueryContext: salsa::QueryContext {
    fn interner(&self) -> &Interner;
}

#[derive(Clone, Default)]
pub struct Interner;
