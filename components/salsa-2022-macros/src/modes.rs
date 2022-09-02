use std::marker::PhantomData;

/// The four possible modes of Salsa structs
/// Salsa structs asre generic over AllowedModes. 
pub(crate) trait AllowedModes {
    const TRACKED: bool;
    const INPUT: bool;
    const INTERNED: bool;
    const ACCUMULATOR: bool;
}

/// 
pub(crate) struct Mode<M: AllowedModes> {
    pub(super) phantom: PhantomData<M>,
}

impl<M: AllowedModes> Default for Mode<M> {
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}


impl<M: AllowedModes> Mode<M> {
    pub(crate) fn singleton_allowed(&self) -> bool {
        M::INPUT
    }
} 

