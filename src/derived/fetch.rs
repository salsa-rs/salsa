use arc_swap::Guard;

use crate::{
    plumbing::{DatabaseOps, QueryFunction},
    runtime::{local_state::QueryInputs, StampedValue},
    Database, QueryDb,
};

use super::{DerivedKeyIndex, DerivedStorage, MemoizationPolicy};

impl<Q, MP> DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    #[inline]
    pub(super) fn fetch(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        key_index: DerivedKeyIndex,
    ) -> Q::Value {
        db.unwind_if_cancelled();

        let StampedValue {
            value,
            durability,
            changed_at,
        } = self.compute_value(db, key_index);

        if let Some(evicted) = self.lru.record_use(key_index) {
            self.evict(evicted);
        }

        db.salsa_runtime()
            .report_query_read_and_unwind_if_cycle_resulted(
                self.database_key_index(key_index),
                durability,
                changed_at,
            );

        value
    }

    #[inline]
    fn compute_value(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        key_index: DerivedKeyIndex,
    ) -> StampedValue<Q::Value> {
        loop {
            if let Some(value) = self
                .fetch_hot(db, key_index)
                .or_else(|| self.fetch_cold(db, key_index))
            {
                return value;
            }
        }
    }

    #[inline]
    fn fetch_hot(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        key_index: DerivedKeyIndex,
    ) -> Option<StampedValue<Q::Value>> {
        let memo_guard = self.memo_map.get(key_index);
        if let Some(memo) = &memo_guard {
            if let Some(value) = &memo.value {
                let runtime = db.salsa_runtime();
                if self.shallow_verify_memo(db, runtime, self.database_key_index(key_index), memo) {
                    return Some(memo.revisions.stamped_value(value.clone()));
                }
            }
        }
        None
    }

    fn fetch_cold(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        key_index: DerivedKeyIndex,
    ) -> Option<StampedValue<Q::Value>> {
        let runtime = db.salsa_runtime();
        let database_key_index = self.database_key_index(key_index);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = self.sync_map.claim(db.ops_database(), database_key_index)?;

        // Push the query on the stack.
        let active_query = runtime.push_query(database_key_index);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        // This time we can do a *deep* verify. Because this can recurse, don't hold the arcswap guard.
        let opt_old_memo = self.memo_map.get(key_index).map(Guard::into_inner);
        if let Some(old_memo) = &opt_old_memo {
            if let Some(value) = &old_memo.value {
                if self.deep_verify_memo(db, old_memo, &active_query) {
                    return Some(old_memo.revisions.stamped_value(value.clone()));
                }
            }
        }

        Some(self.execute(db, active_query, opt_old_memo))
    }

    fn evict(&self, key_index: DerivedKeyIndex) {
        if let Some(memo) = self.memo_map.get(key_index) {
            // Careful: we can't evict memos with untracked inputs
            // as their values cannot be reconstructed.
            if let QueryInputs::Untracked = memo.revisions.inputs {
                return;
            }

            self.memo_map.remove(key_index);
        }
    }
}
