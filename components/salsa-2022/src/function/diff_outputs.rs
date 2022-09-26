use crate::{
    hash::FxHashSet, key::DependencyIndex, runtime::local_state::QueryRevisions,
    storage::HasJarsDyn, Database, DatabaseKeyIndex, Event, EventKind,
};

use super::{memo::Memo, Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Compute the old and new outputs and invoke the `clear_stale_output` callback
    /// for each output that was generated before but is not generated now.
    pub(super) fn diff_outputs(
        &self,
        db: &DynDb<'_, C>,
        key: DatabaseKeyIndex,
        old_memo: &Memo<C::Value>,
        revisions: &QueryRevisions,
    ) {
        // Iterate over the outputs of the `old_memo` and put them into a hashset
        let mut old_outputs = FxHashSet::default();
        old_memo.revisions.origin.outputs().for_each(|i| {
            old_outputs.insert(i);
        });

        // Iterate over the outputs of the current query
        // and remove elements from `old_outputs` when we find them
        for new_output in revisions.origin.outputs() {
            if old_outputs.contains(&new_output) {
                old_outputs.remove(&new_output);
            }
        }

        for old_output in old_outputs {
            Self::report_stale_output(db, key, old_output);
        }
    }

    fn report_stale_output(db: &DynDb<'_, C>, key: DatabaseKeyIndex, output: DependencyIndex) {
        let runtime_id = db.runtime().id();
        db.salsa_event(Event {
            runtime_id,
            kind: EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            },
        });

        db.remove_stale_output(key, output);
    }
}
