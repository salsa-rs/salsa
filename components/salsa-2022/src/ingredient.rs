use crate::{
    cycle::CycleRecoveryStrategy, key::DependencyIndex, runtime::local_state::QueryInputs, Id,
};

use super::Revision;

pub trait Ingredient<DB: ?Sized> {
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy;

    fn maybe_changed_after(&self, db: &DB, input: DependencyIndex, revision: Revision) -> bool;

    fn inputs(&self, key_index: Id) -> Option<QueryInputs>;
}

/// Optional trait for ingredients that wish to be notified when new revisions are
/// about to occur. If ingredients wish to receive these method calls,
/// they need to indicate that by invoking [`Ingredients::push_mut`] during initialization.
pub trait MutIngredient<DB: ?Sized>: Ingredient<DB> {
    /// Invoked when a new revision is about to start. This gives ingredients
    /// a chance to flush data and so forth.
    fn reset_for_new_revision(&mut self);
}
