use crate::{function::memo::MemoConfigured, zalsa_local::QueryRevisions};

use super::{Configuration, IngredientImpl, LruChoice};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// If the value/durability of this memo is equal to what is found in `revisions`/`value`,
    /// then update `revisions.changed_at` to match `self.revisions.changed_at`. This is invoked
    /// on an old memo when a new memo has been produced to check whether there have been changed.
    pub(super) fn backdate_if_appropriate(
        &self,
        old_memo: &MemoConfigured<'_, C>,
        revisions: &mut QueryRevisions,
        value: &C::Output<'_>,
    ) {
        C::Lru::with_value(&old_memo.value, |old_value| {
            // Careful: if the value became less durable than it
            // used to be, that is a "breaking change" that our
            // consumers must be aware of. Becoming *more* durable
            // is not. See the test `constant_to_non_constant`.
            if revisions.durability >= old_memo.revisions.durability
                && C::should_backdate_value(old_value, value)
            {
                tracing::debug!(
                    "value is equal, back-dating to {:?}",
                    old_memo.revisions.changed_at,
                );

                assert!(old_memo.revisions.changed_at <= revisions.changed_at);
                revisions.changed_at = old_memo.revisions.changed_at;
            }
        })
    }
}
