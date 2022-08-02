use arc_swap::Guard;

use crate::{database::AsSalsaDatabase, runtime::StampedValue, AsId, Database};

use super::{Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub fn fetch(&self, db: &DynDb<C>, key: C::Key) -> &C::Value {
        let runtime = db.salsa_runtime();

        runtime.unwind_if_revision_cancelled(db);

        let StampedValue {
            value,
            durability,
            changed_at,
        } = self.compute_value(db, key);

        if let Some(evicted) = self.lru.record_use(key.as_id()) {
            self.evict(AsId::from_id(evicted));
        }

        db.salsa_runtime().report_tracked_read(
            self.database_key_index(key).into(),
            durability,
            changed_at,
        );

        value
    }

    #[inline]
    fn compute_value(&self, db: &DynDb<C>, key: C::Key) -> StampedValue<&C::Value> {
        loop {
            if let Some(value) = self.fetch_hot(db, key).or_else(|| self.fetch_cold(db, key)) {
                return value;
            }
        }
    }

    #[inline]
    fn fetch_hot(&self, db: &DynDb<C>, key: C::Key) -> Option<StampedValue<&C::Value>> {
        let memo_guard = self.memo_map.get(key);
        if let Some(memo) = &memo_guard {
            if memo.value.is_some() {
                let runtime = db.salsa_runtime();
                if self.shallow_verify_memo(db, runtime, self.database_key_index(key), memo) {
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

    fn fetch_cold(&self, db: &DynDb<C>, key: C::Key) -> Option<StampedValue<&C::Value>> {
        let runtime = db.salsa_runtime();
        let database_key_index = self.database_key_index(key);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = self
            .sync_map
            .claim(db.as_salsa_database(), database_key_index)?;

        // Push the query on the stack.
        let active_query = runtime.push_query(database_key_index);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        // This time we can do a *deep* verify. Because this can recurse, don't hold the arcswap guard.
        let opt_old_memo = self.memo_map.get(key).map(Guard::into_inner);
        if let Some(old_memo) = &opt_old_memo {
            if old_memo.value.is_some() {
                if self.deep_verify_memo(db, old_memo, &active_query) {
                    let value = unsafe {
                        // Unsafety invariant: memo is present in memo_map.
                        self.extend_memo_lifetime(old_memo).unwrap()
                    };
                    return Some(old_memo.revisions.stamped_value(value));
                }
            }
        }

        Some(self.execute(db, active_query, opt_old_memo))
    }

    fn evict(&self, key: C::Key) {
        if let Some(memo) = self.memo_map.get(key) {
            // Careful: we can't evict memos with untracked inputs
            // as their values cannot be reconstructed.
            if memo.revisions.inputs.untracked {
                return;
            }

            if let Some(memo) = self.memo_map.remove(key) {
                self.deleted_entries.push(memo);
            }
        }
    }
}
