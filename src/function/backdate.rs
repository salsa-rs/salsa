use crate::zalsa_local::QueryRevisions;

use super::{memo::Memo, Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// If the value/durability of this memo is equal to what is found in `revisions`/`value`,
    /// then updates `revisions.changed_at` to match `self.revisions.changed_at`. This is invoked
    /// on an old memo when a new memo has been produced to check whether there have been changed.
    pub(super) fn backdate_if_appropriate(
        &self,
        old_memo: &Memo<C::Output<'_>>,
        revisions: &mut QueryRevisions,
        value: &C::Output<'_>,
    ) {
        if let Some(old_value) = &old_memo.value {
            // Careful: if the value became less durable than it
            // used to be, that is a "breaking change" that our
            // consumers must be aware of. Becoming *more* durable
            // is not. See the test `constant_to_non_constant`.
            if revisions.durability >= old_memo.revisions.durability
                && C::values_equal(old_value, value)
            {
                tracing::debug!(
                    "value is equal, back-dating to {:?}",
                    old_memo.revisions.changed_at,
                );

                assert!(old_memo.revisions.changed_at <= revisions.changed_at);
                revisions.changed_at = old_memo.revisions.changed_at;
            }
        }
    }
}
