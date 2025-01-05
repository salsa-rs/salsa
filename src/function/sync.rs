use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::ThreadId,
};

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::{
    key::DatabaseKeyIndex, runtime::WaitResult, zalsa::Zalsa, zalsa_local::ZalsaLocal, Database, Id,
};

/// Tracks the keys that are currently being processed; used to coordinate between
/// worker threads.
#[derive(Default)]
pub(crate) struct SyncTable {
    syncs: Mutex<FxHashMap<Id, SyncState>>,
}

struct SyncState {
    id: ThreadId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: AtomicBool,
}

impl SyncTable {
    pub(crate) fn try_claim<'me>(
        &'me self,
        db: &'me dyn Database,
        zalsa_local: &ZalsaLocal,
        zalsa: &'me Zalsa,
        database_key_index: DatabaseKeyIndex,
        id: Id,
    ) -> Option<ClaimGuard<'me>> {
        let mut write = self.syncs.lock();
        match write.entry(id) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let &mut SyncState {
                    id,
                    ref anyone_waiting,
                } = occupied_entry.into_mut();
                // NB: `Ordering::Relaxed` is sufficient here,
                // as there are no loads that are "gated" on this
                // value. Everything that is written is also protected
                // by a lock that must be acquired. The role of this
                // boolean is to decide *whether* to acquire the lock,
                // not to gate future atomic reads.
                anyone_waiting.store(true, Ordering::Relaxed);
                zalsa.block_on_or_unwind(db, zalsa_local, database_key_index, id, write);
                None
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: std::thread::current().id(),
                    anyone_waiting: AtomicBool::new(false),
                });
                Some(ClaimGuard {
                    database_key_index,
                    id,
                    zalsa,
                    sync_table: self,
                })
            }
        }
    }
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub(crate) struct ClaimGuard<'me> {
    database_key_index: DatabaseKeyIndex,
    id: Id,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
}

impl ClaimGuard<'_> {
    fn remove_from_map_and_unblock_queries(&self, wait_result: WaitResult) {
        let mut syncs = self.sync_table.syncs.lock();

        let SyncState { anyone_waiting, .. } = syncs.remove(&self.id).unwrap();

        drop(syncs);

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
