use crate::{
    key::DependencyIndex, runtime::local_state::QueryRevisions, Database, DatabaseKeyIndex, Event,
    EventKind,
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
        let mut old_outputs = old_memo
            .revisions
            .edges
            .outputs()
            .iter()
            .copied()
            .peekable();
        let mut new_outputs = revisions.edges.outputs().iter().copied().peekable();

        // two list are in sorted order, we can merge them in linear time.
        while let (Some(&old_output), Some(&new_output)) = (old_outputs.peek(), new_outputs.peek())
        {
            if old_output < new_output {
                // Output that was generated but is no longer.
                Self::report_stale_output(db, key, old_output);
                old_outputs.next();
            } else if new_output < old_output {
                // This is a new output that was not generated before.
                // No action needed.
                new_outputs.next();
            } else {
                // Output generated both times.
                old_outputs.next();
                new_outputs.next();
            }
        }

        for old_output in old_outputs {
            Self::report_stale_output(db, key, old_output);
        }
    }

    fn report_stale_output(db: &DynDb<'_, C>, key: DatabaseKeyIndex, output: DependencyIndex) {
        let runtime_id = db.salsa_runtime().id();
        db.salsa_event(Event {
            runtime_id,
            kind: EventKind::WillDiscardStaleOutput {
                execute_key: key,
                output_key: output,
            },
        });
    }
}
