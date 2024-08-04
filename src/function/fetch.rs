use arc_swap::Guard;

use crate::{
    runtime::StampedValue, zalsa::ZalsaDatabase, zalsa_local::ZalsaLocal, AsDynDatabase as _, Id,
};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, key: Id) -> &C::Output<'db> {
        let zalsa_local = db.zalsa_local();
        zalsa_local.unwind_if_revision_cancelled(db.as_dyn_database());

        let StampedValue {
            value,
            durability,
            changed_at,
        } = self.compute_value(db, zalsa_local, key);

        if let Some(evicted) = self.lru.record_use(key) {
            self.evict(evicted);
        }

        zalsa_local.report_tracked_read(
            self.database_key_index(key).into(),
            durability,
            changed_at,
        );

        value
    }

    #[inline]
    fn compute_value<'db>(
        &'db self,
        db: &'db C::DbView,
        local_state: &ZalsaLocal,
        key: Id,
    ) -> StampedValue<&'db C::Output<'db>> {
        loop {
            if let Some(value) = self
                .fetch_hot(db, key)
                .or_else(|| self.fetch_cold(db, local_state, key))
            {
                return value;
            }
        }
    }

    #[inline]
    fn fetch_hot<'db>(
        &'db self,
        db: &'db C::DbView,
        key: Id,
    ) -> Option<StampedValue<&'db C::Output<'db>>> {
        let memo_guard = self.memo_map.get(key);
        if let Some(memo) = &memo_guard {
            if memo.value.is_some() {
                let zalsa = db.zalsa();
                if self.shallow_verify_memo(db, zalsa, self.database_key_index(key), memo) {
                    let value = unsafe {
                        // Unsafety invariant: memo is present in memo_map
                        self.extend_memo_lifetime(memo).unwrap()
                    };
                    return Some(memo.revisions.stamped_value(value));
                }
            }
        }
        None
    }

    fn fetch_cold<'db>(
        &'db self,
        db: &'db C::DbView,
        local_state: &ZalsaLocal,
        key: Id,
    ) -> Option<StampedValue<&'db C::Output<'db>>> {
        let database_key_index = self.database_key_index(key);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard =
            self.sync_map
                .claim(db.as_dyn_database(), local_state, database_key_index)?;

        // Push the query on the stack.
        let active_query = local_state.push_query(database_key_index);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        // This time we can do a *deep* verify. Because this can recurse, don't hold the arcswap guard.
        let opt_old_memo = self.memo_map.get(key).map(Guard::into_inner);
        if let Some(old_memo) = &opt_old_memo {
            if old_memo.value.is_some() && self.deep_verify_memo(db, old_memo, &active_query) {
                let value = unsafe {
                    // Unsafety invariant: memo is present in memo_map.
                    self.extend_memo_lifetime(old_memo).unwrap()
                };
                return Some(old_memo.revisions.stamped_value(value));
            }
        }

        Some(self.execute(db, active_query, opt_old_memo))
    }

    fn evict(&self, key: Id) {
        self.memo_map.evict(key);
    }
}
