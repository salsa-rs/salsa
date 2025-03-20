use super::{memo::Memo, Configuration, IngredientImpl};
use crate::{
    hash::FxHashSet, zalsa::Zalsa, zalsa_local::QueryRevisions, AsDynDatabase as _, Database,
    DatabaseKeyIndex, Event, EventKind,
};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke `remove_stale_output`
    /// for each output that was generated before but is not generated now.
    ///
    /// This function takes a `&mut` reference to `revisions` to remove outputs
    /// that no longer exist in this revision from [`QueryRevisions::tracked_struct_ids`].
    ///
    /// If `provisional` is true, the new outputs are from a cycle-provisional result. In
    /// that case, we won't panic if we see outputs from the current revision become stale.
    pub(super) fn diff_outputs(
        &self,
        zalsa: &Zalsa,
        db: &C::DbView,
        key: DatabaseKeyIndex,
        old_memo: &Memo<C::Output<'_>>,
        revisions: &mut QueryRevisions,
        provisional: bool,
    ) {
        // Iterate over the outputs of the `old_memo` and put them into a hashset
        let mut old_outputs: FxHashSet<_> = old_memo.revisions.origin.outputs().collect();
        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.outputs() {
            old_outputs.remove(&new_output);
        }

        if let Some(tracked_struct_ids) = &mut revisions.tracked_struct_ids {
            if !old_outputs.is_empty() {
                // Remove the outputs that are no longer present in the current revision
                // to prevent that the next revision is seeded with a id mapping that no longer exists.
                tracked_struct_ids.retain(|&k, &mut value| {
                    !old_outputs.contains(&DatabaseKeyIndex::new(k.ingredient_index(), value))
                });
            }
            if tracked_struct_ids.is_empty() {
                revisions.tracked_struct_ids = None;
            }
        }

        for old_output in old_outputs {
            Self::report_stale_output(zalsa, db, key, old_output, provisional);
        }
    }

    fn report_stale_output(
        zalsa: &Zalsa,
        db: &C::DbView,
        key: DatabaseKeyIndex,
        output: DatabaseKeyIndex,
        provisional: bool,
    ) {
        db.salsa_event(&|| {
            Event::new(EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            })
        });
        output.remove_stale_output(zalsa, db.as_dyn_database(), key, provisional);
    }
}
