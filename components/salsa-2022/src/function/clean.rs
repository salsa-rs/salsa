use crate::{runtime::local_state::QueryOrigin, Database, DatabaseKeyIndex};

use super::{Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Invoked when the value for `key` *was* set as an output
    /// in a prior revision by `executor`, but in the current revision is was
    /// never assigned a value. In this case, we want to throw away
    /// the old memo, assuming that it is still in place.
    pub(super) fn clean(&self, db: &DynDb<'_, C>, executor: DatabaseKeyIndex, key: C::Key) {
        let runtime = db.salsa_runtime();
        let current_revision = runtime.current_revision();

        let memo = match self.memo_map.get(key) {
            Some(m) => m,
            None => return,
        };

        match memo.revisions.origin {
            QueryOrigin::Assigned(Some(by_query)) => {
                // This memo IS for a value that was assigned by someone.
                // It *must* be the stale value that was assigned by `executor`
                // in some previous revision:
                //
                // Only the query Q that created the tracked struct K can assign a value,
                //   and we are getting this callback because Q did not create K.
                //
                // But can't a tracked struct key K be reused across revisions?
                // Yes! But only after the creator is re-executed and does not
                // create K, in which case K becomes a stale output and is
                // recycled. And when *that* happens, this data would also
                // have been removed.
                assert_eq!(executor, by_query);
                assert!(memo.verified_at.load() < current_revision);

                // Delete the stale key -- but this is not optimal for re-use.
                // We may find that when the value for key is computed,
                // it yields the same value which was previously stored.
                // To fix this, we could set a flag marking this result as
                // "stale" instead of deleting it altogether.
                self.delete_memo(key);
            }
            _ => {
                // This memo is not for the value that was assigned,
                // so that implies the value has been updated in the current revision
                // in some way. Leave it alone.
                assert_eq!(memo.verified_at.load(), current_revision);
            }
        }
    }
}
