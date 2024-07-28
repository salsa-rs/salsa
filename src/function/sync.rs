use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::ThreadId,
};

use crate::{
    hash::FxDashMap, key::DatabaseKeyIndex, runtime::WaitResult, zalsa::Zalsa,
    zalsa_local::ZalsaLocal, Database, Id,
};

#[derive(Default)]
pub(super) struct SyncMap {
    sync_map: FxDashMap<Id, SyncState>,
}

struct SyncState {
    id: ThreadId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: AtomicBool,
}

impl SyncMap {
    pub(super) fn claim<'me>(
        &'me self,
        db: &'me dyn Database,
        local_state: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
    ) -> Option<ClaimGuard<'me>> {
        let zalsa = db.zalsa();
        let thread_id = std::thread::current().id();
        match self.sync_map.entry(database_key_index.key_index) {
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(SyncState {
                    id: thread_id,
                    anyone_waiting: AtomicBool::new(false),
                });
                Some(ClaimGuard {
                    database_key: database_key_index,
                    zalsa,
                    sync_map: &self.sync_map,
                })
            }
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                // NB: `Ordering::Relaxed` is sufficient here,
                // as there are no loads that are "gated" on this
                // value. Everything that is written is also protected
                // by a lock that must be acquired. The role of this
                // boolean is to decide *whether* to acquire the lock,
                // not to gate future atomic reads.
                entry.get().anyone_waiting.store(true, Ordering::Relaxed);
                let other_id = entry.get().id;
                zalsa.block_on_or_unwind(db, local_state, database_key_index, other_id, entry);
                None
            }
        }
    }
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub(super) struct ClaimGuard<'me> {
    database_key: DatabaseKeyIndex,
    zalsa: &'me Zalsa,
    sync_map: &'me FxDashMap<Id, SyncState>,
}

impl<'me> ClaimGuard<'me> {
    fn remove_from_map_and_unblock_queries(&self, wait_result: WaitResult) {
        let (_, SyncState { anyone_waiting, .. }) =
            self.sync_map.remove(&self.database_key.key_index).unwrap();

        // NB: `Ordering::Relaxed` is sufficient here,
        // see `store` above for explanation.
        if anyone_waiting.load(Ordering::Relaxed) {
            self.zalsa
                .unblock_queries_blocked_on(self.database_key, wait_result)
        }
    }
}

impl<'me> Drop for ClaimGuard<'me> {
    fn drop(&mut self) {
        let wait_result = if std::thread::panicking() {
            WaitResult::Panicked
        } else {
            WaitResult::Completed
        };
        self.remove_from_map_and_unblock_queries(wait_result)
    }
}
