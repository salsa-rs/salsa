use super::{memo::Memo, Configuration, IngredientImpl};
use crate::{
    hash::FxHashSet, zalsa_local::QueryRevisions, AsDynDatabase as _, DatabaseKeyIndex, Event,
    EventKind,
};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke the `clear_stale_output` callback
    /// for each output that was generated before but is not generated now.
    ///
    /// This function takes a `&mut` reference to `revisions` to remove outputs
    /// that no longer exist in this revision from [`QueryRevisions::tracked_struct_ids`].
    pub(super) fn diff_outputs(
        &self,
        db: &C::DbView,
        key: DatabaseKeyIndex,
        old_memo: &Memo<C::Output<'_>>,
        revisions: &mut QueryRevisions,
    ) {
        // Iterate over the outputs of the `old_memo` and put them into a hashset
        let mut old_outputs: FxHashSet<_> = old_memo.revisions.origin.outputs().collect();

        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.outputs() {
            old_outputs.remove(&new_output);
        }

        if !old_outputs.is_empty() {
            // Remove the outputs that are no longer present in the current revision
            // to prevent that the next revision is seeded with a id mapping that no longer exists.
            revisions.tracked_struct_ids.retain(|k, value| {
                !old_outputs.contains(&DatabaseKeyIndex {
                    ingredient_index: k.ingredient_index(),
                    key_index: *value,
                })
            });
        }

        for old_output in old_outputs {
            Self::report_stale_output(db, key, old_output);
        }
    }

    fn report_stale_output(db: &C::DbView, key: DatabaseKeyIndex, output: DatabaseKeyIndex) {
        let db = db.as_dyn_database();

        db.salsa_event(&|| Event {
            thread_id: std::thread::current().id(),
            kind: EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            },
        });

        output.remove_stale_output(db, key);
    }
}
