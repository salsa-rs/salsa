use std::thread::ThreadId;

use parking_lot::Mutex;

use crate::{
    key::DatabaseKeyIndex,
    runtime::BlockResult,
    zalsa::{MemoIngredientIndex, Zalsa},
    Database,
};

use super::util;

/// Tracks the keys that are currently being processed; used to coordinate between
/// worker threads.
#[derive(Default)]
pub(crate) struct SyncTable {
    syncs: Mutex<Vec<Option<SyncState>>>,
}

struct SyncState {
    id: ThreadId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: bool,
}

pub(crate) enum ClaimResult<'a> {
    Retry,
    Cycle,
    Claimed(ClaimGuard<'a>),
}

impl SyncTable {
    #[inline]
    pub(crate) fn claim<'me>(
        &'me self,
        db: &'me (impl ?Sized + Database),
        zalsa: &'me Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> ClaimResult<'me> {
        let mut syncs = self.syncs.lock();
        let thread_id = std::thread::current().id();

        util::ensure_vec_len(&mut syncs, memo_ingredient_index.as_usize() + 1);

        match &mut syncs[memo_ingredient_index.as_usize()] {
            None => {
                syncs[memo_ingredient_index.as_usize()] = Some(SyncState {
                    id: thread_id,
                    anyone_waiting: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    database_key_index,
                    memo_ingredient_index,
                    zalsa,
                    sync_table: self,
                    _padding: false,
                })
            }
            Some(SyncState {
                id: other_id,
                anyone_waiting,
            }) => {
                // NB: `Ordering::Relaxed` is sufficient here,
                // as there are no loads that are "gated" on this
                // value. Everything that is written is also protected
                // by a lock that must be acquired. The role of this
                // boolean is to decide *whether* to acquire the lock,
                // not to gate future atomic reads.
                *anyone_waiting = true;
                match zalsa.runtime().block_on(
                    db.as_dyn_database(),
                    db.zalsa_local(),
                    database_key_index,
                    *other_id,
                    syncs,
                ) {
                    BlockResult::Completed => ClaimResult::Retry,
                    BlockResult::Cycle => ClaimResult::Cycle,
                }
            }
        }
    }
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub(crate) struct ClaimGuard<'me> {
    database_key_index: DatabaseKeyIndex,
    memo_ingredient_index: MemoIngredientIndex,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
    // Reduce the size of ClaimResult by making more niches available in ClaimGuard; this fits into
    // the padding of ClaimGuard so doesn't increase its size.
    _padding: bool,
}

impl ClaimGuard<'_> {
    fn remove_from_map_and_unblock_queries(&self) {
        let mut syncs = self.sync_table.syncs.lock();

        let SyncState { anyone_waiting, .. } =
            syncs[self.memo_ingredient_index.as_usize()].take().unwrap();

        if anyone_waiting {
            self.zalsa
                .runtime()
                .unblock_queries_blocked_on(self.database_key_index)
        }
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        self.remove_from_map_and_unblock_queries()
    }
}

impl std::fmt::Debug for SyncTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncTable").finish()
    }
}
