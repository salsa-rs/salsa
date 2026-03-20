use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::zalsa_local::QueryRevisions;
use crate::DatabaseKeyIndex;

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// If the value/durability of this memo is equal to what is found in `revisions`/`value`,
    /// then update `revisions.changed_at` to match `self.revisions.changed_at`. This is invoked
    /// on an old memo when a new memo has been produced to check whether there have been changed.
    pub(super) fn backdate_if_appropriate<'db>(
        &self,
        old_memo: &Memo<'db, C>,
        index: DatabaseKeyIndex,
        revisions: &mut QueryRevisions,
        value: &C::Output<'db>,
    ) {
        // We've seen issues where queries weren't re-validated when backdating provisional values
        // in ty. This is more of a bandaid because we're close to a release and don't have the time to prove
        // right now whether backdating could be made safe for queries participating in queries.
        // TODO: Write a test that demonstrates that backdating queries participating in a cycle isn't safe
        // OR write many tests showing that it is (and fixing the case where it didn't correctly account for today).
        if !revisions.cycle_heads().is_empty() || old_memo.may_be_provisional() {
            return;
        }

        if let Some(old_value) = &old_memo.value {
            // Careful: if the value became less durable than it
            // used to be, that is a "breaking change" that our
            // consumers must be aware of. Becoming *more* durable
            // is not. See the test `durable_to_less_durable`.
            if revisions.durability >= old_memo.revisions.durability
                && C::values_equal(old_value, value)
            {
                crate::tracing::debug!(
                    "{index:?} value is equal, back-dating to {:?}",
                    old_memo.revisions.changed_at,
                );

                if old_memo.revisions.changed_at > revisions.changed_at {
                    let message = format_args!(
                        "query {index:?} returned the same value, but the previous execution \
                         changed at {:?} and the new execution changed at {:?}. This usually \
                         means the query re-executed because an input changed, but then branched \
                         on untracked state (for example, a global variable, a non-salsa field \
                         on the database, or filesystem state read outside salsa) and no longer \
                         read that input. This is usually a bug in the query implementation. \
                         Queries that branch on untracked state can also produce stale results. \
                         If the query has no untracked reads, please open a salsa issue.",
                        old_memo.revisions.changed_at, revisions.changed_at,
                    );

                    if cfg!(debug_assertions) {
                        panic!("{message}");
                    } else {
                        crate::tracing::warn!("{message}");
                        // Fallthrough to still use the old memo's changed_at
                        // to ensure `changed_at` is never decreasing.
                    }
                }

                revisions.changed_at = old_memo.revisions.changed_at;
            }
        }
    }
}
