pub trait CompilerDatabase: salsa::Database {
    fn interner(&self) -> &Interner;
}

#[derive(Clone, Default)]
pub struct Interner;
