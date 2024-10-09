use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::ThreadId,
};

use parking_lot::RwLock;

use crate::{
    key::DatabaseKeyIndex,
    runtime::WaitResult,
    zalsa::{MemoIngredientIndex, Zalsa},
    zalsa_local::ZalsaLocal,
    Database,
};

use super::util;

/// Tracks the keys that are currently being processed; used to coordinate between
/// worker threads.
#[derive(Default)]
pub(crate) struct SyncTable {
    syncs: RwLock<Vec<Option<SyncState>>>,
}

struct SyncState {
    id: ThreadId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: AtomicBool,
}

impl SyncTable {
    pub(crate) fn claim<'me>(
        &'me self,
        db: &'me dyn Database,
        zalsa_local: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ClaimGuard<'me>> {
        let mut syncs = self.syncs.write();
        let zalsa = db.zalsa();
        let thread_id = std::thread::current().id();

        util::ensure_vec_len(&mut syncs, memo_ingredient_index.as_usize() + 1);

        match &syncs[memo_ingredient_index.as_usize()] {
            None => {
                syncs[memo_ingredient_index.as_usize()] = Some(SyncState {
                    id: thread_id,
                    anyone_waiting: AtomicBool::new(false),
                });
                Some(ClaimGuard {
                    database_key_index,
                    memo_ingredient_index,
                    zalsa,
                    sync_table: self,
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
                anyone_waiting.store(true, Ordering::Relaxed);
                zalsa.block_on_or_unwind(db, zalsa_local, database_key_index, *other_id, syncs);
                None
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
}

impl ClaimGuard<'_> {
    fn remove_from_map_and_unblock_queries(&self, wait_result: WaitResult) {
        let mut syncs = self.sync_table.syncs.write();

        let SyncState { anyone_waiting, .. } =
            syncs[self.memo_ingredient_index.as_usize()].take().unwrap();

        // NB: `Ordering::Relaxed` is sufficient here,
        // see `store` above for explanation.
        if anyone_waiting.load(Ordering::Relaxed) {
            self.zalsa
                .unblock_queries_blocked_on(self.database_key_index, wait_result)
        }
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        let wait_result = if std::thread::panicking() {
            WaitResult::Panicked
        } else {
            WaitResult::Completed
        };
        self.remove_from_map_and_unblock_queries(wait_result)
    }
}

impl std::fmt::Debug for SyncTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncTable").finish()
    }
}
