use crate::input::{Input, length};
use crate::interned::{InternedInput, interned_length};

#[derive(Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
pub enum SupertypeInput<'db> {
    InternedInput(InternedInput<'db>),
    Input(Input),
}

#[salsa::tracked(returns(copy))]
#[inline(never)]
pub fn either_length<'db>(db: &'db dyn salsa::Database, input: SupertypeInput<'db>) -> usize {
    match input {
        SupertypeInput::InternedInput(input) => interned_length(db, input),
        SupertypeInput::Input(input) => length(db, input),
    }
}
