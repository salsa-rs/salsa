use crate::input::Input;

#[salsa::tracked]
pub struct Tracked<'db> {
    #[returns(copy)]
    pub value: usize,
}

#[salsa::tracked(returns(copy))]
pub fn make_tracked(db: &dyn salsa::Database, input: Input) -> Tracked<'_> {
    Tracked::new(db, input.text(db).len())
}
