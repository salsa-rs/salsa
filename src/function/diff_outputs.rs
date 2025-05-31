use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::hash::FxIndexSet;
use crate::zalsa::Zalsa;
use crate::zalsa_local::QueryRevisions;
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
        old_memo: &Memo<C::Output<'_>>,
        revisions: &mut QueryRevisions,
    ) {
        // Iterate over the outputs of the `old_memo` and put them into a hashset
        //
        // Ignore key_generation here, because we use the same tracked struct allocation for
        // all generations with the same key_index and can't report it as stale
        let mut old_outputs: FxIndexSet<_> = old_memo
            .revisions
            .origin
            .as_ref()
            .outputs()
            .map(|a| (a.ingredient_index(), a.key_index().index()))
            .collect();

        if old_outputs.is_empty() {
            return;
        }

        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.as_ref().outputs() {
            old_outputs.swap_remove(&(
                new_output.ingredient_index(),
                new_output.key_index().index(),
            ));
        }

        if old_outputs.is_empty() {
            return;
        }

        if let Some(tracked_struct_ids) = revisions.tracked_struct_ids_mut() {
            // Remove the outputs that are no longer present in the current revision
            // to prevent that the next revision is seeded with an id mapping that no longer exists.
            tracked_struct_ids
                .retain(|(k, value)| !old_outputs.contains(&(k.ingredient_index(), value.index())));
        }

        for (ingredient_index, key_index) in old_outputs {
            // SAFETY: key_index acquired from valid output
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
