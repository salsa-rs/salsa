use crate::function::{Configuration, IngredientImpl};
use crate::zalsa::Zalsa;
use crate::zalsa_local::ZalsaLocal;

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Returns the value memoized for the active key in the previous revision,
    /// replaying that result's dependencies into the active query.
    ///
    /// # Panics
    ///
    /// Panics if the active query is not an execution of this tracked function.
    pub fn previous<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
    ) -> Option<&'db C::Output<'db>> {
        let Some((database_key_index, _)) = zalsa_local.active_query() else {
            panic!(
                "cannot access previous memoized value for {} outside of its tracked function",
                C::DEBUG_NAME,
            );
        };

        if database_key_index.ingredient_index() != self.index {
            panic!(
                "cannot access previous memoized value for {} while executing {database_key_index:?}",
                C::DEBUG_NAME,
            );
        }

        let id = database_key_index.key_index();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        let memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index)?;

        if memo.header.may_be_provisional() {
            return None;
        }

        let value = memo.value.as_ref()?;
        let revisions = &memo.header.revisions;
        zalsa_local.report_previous_memo_read(
            revisions.durability,
            revisions.origin().edges(),
            revisions.is_derived_untracked(),
            revisions.tracked_struct_ids(),
            #[cfg(feature = "accumulator")]
            revisions.accumulated_inputs.load(),
            zalsa.current_revision(),
        );
        Some(value)
    }
}
