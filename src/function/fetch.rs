use crate::{runtime::StampedValue, zalsa::ZalsaDatabase, AsDynDatabase as _, Id};

use super::{memo::Memo, Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> &'db C::Output<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        zalsa_local.unwind_if_revision_cancelled(db.as_dyn_database());

        let memo = self.refresh_memo(db, id);
        let StampedValue {
            value,
            durability,
            changed_at,
        } = memo.revisions.stamped_value(memo.value.as_ref().unwrap());

        if let Some(evicted) = self.lru.record_use(id) {
            self.evict_value_from_memo_for(zalsa, evicted);
        }

        zalsa_local.report_tracked_read(
            self.database_key_index(id).into(),
            durability,
            changed_at,
            &memo.revisions.cycle_heads,
        );

        value
    }

    #[inline]
    pub(super) fn refresh_memo<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
    ) -> &'db Memo<C::Output<'db>> {
        loop {
            if let Some(memo) = self.fetch_hot(db, id).or_else(|| self.fetch_cold(db, id)) {
                return memo;
            }
        }
    }

    #[inline]
    fn fetch_hot<'db>(&'db self, db: &'db C::DbView, id: Id) -> Option<&'db Memo<C::Output<'db>>> {
        let zalsa = db.zalsa();
        let memo_guard = self.get_memo_from_table_for(zalsa, id);
        if let Some(memo) = &memo_guard {
            if memo.value.is_some()
                && self.shallow_verify_memo(db, zalsa, self.database_key_index(id), memo)
            {
                // Unsafety invariant: memo is present in memo_map
                unsafe {
                    return Some(self.extend_memo_lifetime(memo));
                }
            }
        }
        None
    }

    fn fetch_cold<'db>(&'db self, db: &'db C::DbView, id: Id) -> Option<&'db Memo<C::Output<'db>>> {
        let (zalsa, zalsa_local) = db.zalsas();
        let database_key_index = self.database_key_index(id);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = zalsa.sync_table_for(id).claim(
            db.as_dyn_database(),
            zalsa_local,
            database_key_index,
            self.memo_ingredient_index,
        )?;

        // Push the query on the stack.
        let active_query = zalsa_local.push_query(database_key_index);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let zalsa = db.zalsa();
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id);
        if let Some(old_memo) = &opt_old_memo {
            if old_memo.value.is_some() && self.deep_verify_memo(db, old_memo, &active_query) {
                // Unsafety invariant: memo is present in memo_map.
                unsafe {
                    return Some(self.extend_memo_lifetime(old_memo));
                }
            }
        }

        Some(self.execute(db, active_query, opt_old_memo))
    }
}
