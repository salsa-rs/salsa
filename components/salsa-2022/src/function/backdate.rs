use crate::runtime::local_state::QueryRevisions;

use super::{memo::Memo, Configuration, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// If the value/durability of this memo is equal to what is found in `revisions`/`value`,
    /// then updates `revisions.changed_at` to match `self.revisions.changed_at`. This is invoked
    /// on an old memo when a new memo has been produced to check whether there have been changed.
    pub(super) fn backdate_if_appropriate(
        &self,
        old_memo: &Memo<C::Value>,
        revisions: &mut QueryRevisions,
        value: &C::Value,
    ) {
        if let Some(old_value) = &old_memo.value {
            // Careful: if the value became less durable than it
            // used to be, that is a "breaking change" that our
            // consumers must be aware of. Becoming *more* durable
            // is not. See the test `constant_to_non_constant`.
            if revisions.durability >= old_memo.revisions.durability
                && C::should_backdate_value(old_value, value)
            {
                log::debug!(
                    "value is equal, back-dating to {:?}",
                    old_memo.revisions.changed_at,
                );

                assert!(old_memo.revisions.changed_at <= revisions.changed_at);
                revisions.changed_at = old_memo.revisions.changed_at;
            }
        }
    }
}
