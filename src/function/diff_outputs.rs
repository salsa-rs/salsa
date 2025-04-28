use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::hash::FxIndexSet;
use crate::zalsa::Zalsa;
use crate::zalsa_local::QueryRevisions;
use crate::{AsDynDatabase as _, Database, DatabaseKeyIndex, Event, EventKind};

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
        let mut old_outputs: FxIndexSet<_> = old_memo.revisions.origin.outputs().collect();

        if old_outputs.is_empty() {
            return;
        }

        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.outputs() {
            old_outputs.swap_remove(&new_output);
        }

        if old_outputs.is_empty() {
            return;
        }

        // Remove the outputs that are no longer present in the current revision
        // to prevent that the next revision is seeded with an id mapping that no longer exists.
        revisions.tracked_struct_ids.retain(|&k, &mut value| {
            !old_outputs.contains(&DatabaseKeyIndex::new(k.ingredient_index(), value))
        });

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
