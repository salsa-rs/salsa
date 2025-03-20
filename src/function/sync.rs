use std::thread::ThreadId;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::{
    key::DatabaseKeyIndex,
    runtime::{BlockResult, WaitResult},
    zalsa::Zalsa,
    Database, Id, IngredientIndex,
};

/// Tracks the keys that are currently being processed; used to coordinate between
/// worker threads.
pub(crate) struct SyncTable {
    syncs: Mutex<FxHashMap<Id, SyncState>>,
    ingredient: IngredientIndex,
}

pub(crate) enum ClaimResult<'a> {
    Retry,
    Cycle,
    Claimed(ClaimGuard<'a>),
}

struct SyncState {
    id: ThreadId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: bool,
}

impl SyncTable {
    pub(crate) fn new(ingredient: IngredientIndex) -> Self {
        Self {
            syncs: Default::default(),
            ingredient,
        }
    }

    pub(crate) fn try_claim<'me>(
        &'me self,
        db: &'me (impl ?Sized + Database),
        zalsa: &'me Zalsa,
        key_index: Id,
    ) -> ClaimResult<'me> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let &mut SyncState {
                    id,
                    ref mut anyone_waiting,
                } = occupied_entry.into_mut();
                // NB: `Ordering::Relaxed` is sufficient here,
                // as there are no loads that are "gated" on this
                // value. Everything that is written is also protected
                // by a lock that must be acquired. The role of this
                // boolean is to decide *whether* to acquire the lock,
                // not to gate future atomic reads.
                *anyone_waiting = true;
                match zalsa.runtime().block_on(
                    db,
                    DatabaseKeyIndex::new(self.ingredient, key_index),
                    id,
                    write,
                ) {
                    BlockResult::Completed => ClaimResult::Retry,
                    BlockResult::Cycle => ClaimResult::Cycle,
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: std::thread::current().id(),
                    anyone_waiting: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    sync_table: self,
                    _padding: false,
                })
            }
        }
    }
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub(crate) struct ClaimGuard<'me> {
    key_index: Id,
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
            syncs.remove(&self.key_index).expect("key claimed twice?");

        if anyone_waiting {
            self.zalsa.runtime().unblock_queries_blocked_on(
                DatabaseKeyIndex::new(self.sync_table.ingredient, self.key_index),
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
