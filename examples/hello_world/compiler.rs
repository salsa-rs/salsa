pub trait CompilerQueryContext: salsa::BaseQueryContext {
    fn interner(&self) -> &Interner;
}

#[derive(Clone, Default)]
pub struct Interner;
