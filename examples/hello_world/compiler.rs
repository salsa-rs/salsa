pub trait CompilerQueryContext: salsa::BaseQueryContext {
    fn interner(&self) -> &Interner;
}

#[derive(Clone)]
pub struct Interner;
