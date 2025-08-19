use crate::active_query::CompletedQuery;
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::hash::FxIndexSet;
use crate::zalsa::Zalsa;
use crate::zalsa_local::{output_edges, QueryOriginRef};
use crate::{DatabaseKeyIndex, Event, EventKind};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke `remove_stale_output` for each output that
    /// was generated before but is not generated now.
    pub(super) fn diff_outputs(
        &self,
        zalsa: &Zalsa,
        key: DatabaseKeyIndex,
        old_memo: &Memo<'_, C>,
        completed_query: &CompletedQuery,
    ) {
        let (QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges)) =
            old_memo.revisions.origin.as_ref()
        else {
            return;
        };

        // Note that tracked structs are not stored as direct query outputs, but they are still outputs
        // that need to be reported as stale.
        for (identity, id) in &completed_query.stale_tracked_structs {
            let output = DatabaseKeyIndex::new(identity.ingredient_index(), *id);
            Self::report_stale_output(zalsa, key, output);
        }

        let mut stale_outputs = output_edges(edges).collect::<FxIndexSet<_>>();

        if stale_outputs.is_empty() {
            return;
        }

        // Preserve any outputs that were recreated in the current revision.
        for new_output in completed_query.revisions.origin.as_ref().outputs() {
            stale_outputs.swap_remove(&new_output);
        }

        // Any outputs that were created in a previous revision but not the current one are stale.
        for output in stale_outputs {
            Self::report_stale_output(zalsa, key, output);
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
