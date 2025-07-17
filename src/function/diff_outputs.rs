use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::hash::FxIndexSet;
use crate::zalsa::Zalsa;
use crate::zalsa_local::FullQueryRevisions;
use crate::{DatabaseKeyIndex, Event, EventKind, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke `remove_stale_output`
    /// for each output that was generated before but is not generated now.
    ///
    /// This function takes a `&mut` reference to `revisions` to remove outputs
    /// that no longer exist in this revision from [`QueryRevisions::tracked_struct_ids`].
    pub(super) fn diff_outputs(
        &self,
        zalsa: &Zalsa,
        key: DatabaseKeyIndex,
        old_memo: &Memo<'_, C>,
        revisions: &mut FullQueryRevisions,
    ) {
        // Collect the outputs from the previous execution of the query.
        //
        // Ignore ID generations here, because we use the same tracked struct allocation for
        // all generations with the same ID index. Any ID being reused with a new generation
        // indicates that the cleanup has already been performed for the previous value.
        let mut old_outputs = old_memo
            .revisions
            .tracked_outputs()
            .map(|key| (key.ingredient_index(), key.key_index().index()))
            .collect::<FxIndexSet<_>>();

        if old_outputs.is_empty() {
            return;
        }

        // Remove any elements from `old_outputs` that were recreated in the current revision.
        for new_output in revisions.outputs() {
            old_outputs.swap_remove(&(
                new_output.ingredient_index(),
                new_output.key_index().index(),
            ));
        }

        // Remove the outputs that are no longer present in the current revision, to prevent
        // seeding the next revisions with IDs that no longer exist.
        if let Some(tracked_struct_ids) = revisions.tracked_struct_ids_mut() {
            tracked_struct_ids.retain(|(identity, id)| {
                !old_outputs.contains(&(identity.ingredient_index(), id.index()))
            });
        }

        for (ingredient_index, key_index) in old_outputs {
            // SAFETY: `key_index` was acquired from a valid `Id`.
            let id = unsafe { Id::from_index(key_index) };
            Self::report_stale_output(zalsa, key, DatabaseKeyIndex::new(ingredient_index, id));
        }
    }

    fn report_stale_output(zalsa: &Zalsa, key: DatabaseKeyIndex, output: DatabaseKeyIndex) {
        zalsa.event(&|| {
            Event::new(EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            })
        });
        output.remove_stale_output(zalsa, key);
    }
}
