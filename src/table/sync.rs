use std::thread::ThreadId;

use parking_lot::Mutex;

use crate::{
    key::DatabaseKeyIndex,
    runtime::WaitResult,
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
    #[inline]
    pub(crate) fn claim<'me>(
        db: &'me (impl ?Sized + Database),
        zalsa: &'me Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ClaimGuard<'me>> {
        // SAFETY: We are supplying the correct current revision
        let sync_table = unsafe {
            zalsa
                .table()
                .syncs(database_key_index.key_index, zalsa.current_revision())
        };
        let mut syncs = sync_table.syncs.lock();
        let thread_id = std::thread::current().id();

        util::ensure_vec_len(&mut syncs, memo_ingredient_index.as_usize() + 1);

        match &mut syncs[memo_ingredient_index.as_usize()] {
            None => {
                syncs[memo_ingredient_index.as_usize()] = Some(SyncState {
                    id: thread_id,
                    anyone_waiting: false,
                });
                Some(ClaimGuard {
                    database_key_index,
                    memo_ingredient_index,
                    zalsa,
                    sync_table,
                })
            }
            Some(SyncState {
                id: other_id,
                anyone_waiting,
            }) => {
                *anyone_waiting = true;
                zalsa.runtime().block_on_or_unwind(
                    db.as_dyn_database(),
                    db.zalsa_local(),
                    database_key_index,
                    *other_id,
                    syncs,
                );
                None
            }
        }
    }

    fn remove_from_map_and_unblock_queries(&self) {
        let mut syncs = self.sync_table.syncs.lock();

        let SyncState { anyone_waiting, .. } =
            syncs[self.memo_ingredient_index.as_usize()].take().unwrap();

        if anyone_waiting {
            self.zalsa.runtime().unblock_queries_blocked_on(
                self.database_key_index,
                if std::thread::panicking() {
                    WaitResult::Panicked
                } else {
                    WaitResult::Completed
                },
            )
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
