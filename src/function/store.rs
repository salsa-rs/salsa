use std::sync::Arc;

use crossbeam::atomic::AtomicCell;

use crate::{
    durability::Durability,
    zalsa_local::{QueryOrigin, QueryRevisions},
    Id, Runtime,
};

use super::{memo::Memo, Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn store<'db>(
        &'db mut self,
        runtime: &mut Runtime,
        key: Id,
        value: C::Output<'db>,
        durability: Durability,
    ) {
        let revision = runtime.current_revision();
        let memo = Memo {
            value: Some(value),
            verified_at: AtomicCell::new(revision),
            revisions: QueryRevisions {
                changed_at: revision,
                durability,
                origin: QueryOrigin::BaseInput,
            },
        };

        if let Some(old_value) = self.memo_map.insert(key, Arc::new(memo)) {
            // NB: we don't have to store `old_value` into `deleted_entries` because we have `&mut self`.
            let durability = old_value.load().revisions.durability;
            runtime.report_tracked_write(durability);
        }
    }
}
