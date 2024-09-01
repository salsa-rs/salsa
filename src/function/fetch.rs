use crate::{runtime::StampedValue, zalsa::ZalsaDatabase, AsDynDatabase as _, Id};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> crate::Result<&C::Output<'db>> {
        let (zalsa, zalsa_local) = db.zalsas();
        zalsa_local.unwind_if_revision_cancelled(db.as_dyn_database())?;

        let StampedValue {
            value,
            durability,
            changed_at,
        } = self.compute_value(db, id)?;

        if let Some(evicted) = self.lru.record_use(id) {
            self.evict_value_from_memo_for(zalsa, evicted);
        }

        zalsa_local.report_tracked_read(
            self.database_key_index(id).into(),
            durability,
            changed_at,
        )?;

        Ok(value)
    }

    #[inline]
    fn compute_value<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
    ) -> crate::Result<StampedValue<&'db C::Output<'db>>> {
        loop {
            if let Some(value) = self.fetch_hot(db, id) {
                return Ok(value);
            }

            if let Some(value) = self.fetch_cold(db, id)? {
                return Ok(value);
            }
        }
    }

    #[inline]
    fn fetch_hot<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
    ) -> Option<StampedValue<&'db C::Output<'db>>> {
        let zalsa = db.zalsa();
        let memo_guard = self.get_memo_from_table_for(zalsa, id);
        if let Some(memo) = &memo_guard {
            if memo.value.is_some()
                && self.shallow_verify_memo(db, zalsa, self.database_key_index(id), memo)
            {
                let value = unsafe {
                    // Unsafety invariant: memo is present in memo_map
                    self.extend_memo_lifetime(memo).unwrap()
                };
                return Some(memo.revisions.stamped_value(value));
            }
        }
        None
    }

    fn fetch_cold<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
    ) -> crate::Result<Option<StampedValue<&'db C::Output<'db>>>> {
        let (zalsa, zalsa_local) = db.zalsas();
        let database_key_index = self.database_key_index(id);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        // FIXME: Handle error
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
            if old_memo.value.is_some() {
                match self.deep_verify_memo(db, old_memo, &active_query) {
                    Ok(true) => {
                        let value = unsafe {
                            // Unsafety invariant: memo is present in memo_map.
                            self.extend_memo_lifetime(old_memo).unwrap()
                        };
                        return Ok(Some(old_memo.revisions.stamped_value(value)));
                    }
                    Err(error) => return Err(error),
                    _ => {}
                }
            }
        }

        self.execute(db, active_query, opt_old_memo).map(Some)
    }
}
