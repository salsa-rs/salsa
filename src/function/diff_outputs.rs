use super::{memo::Memo, Configuration, IngredientImpl};
use crate::{
    key::OutputDependencyIndex, plumbing::ZalsaLocal, zalsa::Zalsa, zalsa_local::QueryRevisions,
    AsDynDatabase as _, Database, DatabaseKeyIndex, Event, EventKind,
};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke the `remove_stale_output` callback
    /// for each output that was generated before but is not generated now.
    ///
    /// This function takes a `&mut` reference to `revisions` to remove outputs
    /// that no longer exist in this revision from [`QueryRevisions::tracked_struct_ids`].
    pub(super) fn diff_outputs(
        &self,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        db: &C::DbView,
        key: DatabaseKeyIndex,
        old_memo: &Memo<C::Output<'_>>,
        revisions: &mut QueryRevisions,
    ) {
        // Iterate over the outputs of the `old_memo` and put them into a hashset
        let old_outputs = &mut *zalsa_local.diff_outputs_scratch.borrow_mut();
        old_outputs.extend(old_memo.revisions.origin.outputs());

        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.outputs() {
            old_outputs.remove(&new_output);
        }

        if !old_outputs.is_empty() {
            // Remove the outputs that are no longer present in the current revision
            // to prevent that the next revision is seeded with a id mapping that no longer exists.
            revisions.tracked_struct_ids.retain(|&k, &mut value| {
                !old_outputs.contains(&OutputDependencyIndex::new(k.ingredient_index(), value))
            });
        }

        for old_output in old_outputs.drain() {
            Self::report_stale_output(zalsa, db, key, old_output);
        }
    }

    fn report_stale_output(
        zalsa: &Zalsa,
        db: &C::DbView,
        key: DatabaseKeyIndex,
        output: OutputDependencyIndex,
    ) {
        db.salsa_event(&|| {
            Event::new(EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            })
        });

        output.remove_stale_output(zalsa, db.as_dyn_database(), key);
    }
}
