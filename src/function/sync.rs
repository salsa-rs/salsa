use rustc_hash::FxHashMap;

use crate::key::DatabaseKeyIndex;
use crate::runtime::{BlockResult, Running, WaitResult};
use crate::sync::thread::{self, ThreadId};
use crate::sync::Mutex;
use crate::zalsa::Zalsa;
use crate::{Id, IngredientIndex};

pub(crate) type SyncGuard<'me> = crate::sync::MutexGuard<'me, FxHashMap<Id, SyncState>>;

/// Tracks the keys that are currently being processed; used to coordinate between
/// worker threads.
pub(crate) struct SyncTable {
    syncs: Mutex<FxHashMap<Id, SyncState>>,
    ingredient: IngredientIndex,
}

pub(crate) enum ClaimResult<'a> {
    /// Can't claim the query because it is running on an other thread.
    Running(Running<'a>),
    /// Claiming the query results in a cycle.
    Cycle,
    /// Successfully claimed the query.
    Claimed(ClaimGuard<'a>),
}

pub(crate) struct SyncState {
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

    pub(crate) fn try_claim<'me>(&'me self, zalsa: &'me Zalsa, key_index: Id) -> ClaimResult<'me> {
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
                match zalsa.runtime().block(
                    DatabaseKeyIndex::new(self.ingredient, key_index),
                    id,
                    write,
                ) {
                    BlockResult::Running(blocked_on) => ClaimResult::Running(blocked_on),
                    BlockResult::Cycle => ClaimResult::Cycle,
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: thread::current().id(),
                    anyone_waiting: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    key_index,
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
    key_index: Id,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
}

impl ClaimGuard<'_> {
    fn remove_from_map_and_unblock_queries(&self) {
        let mut syncs = self.sync_table.syncs.lock();

        let SyncState { anyone_waiting, .. } =
            syncs.remove(&self.key_index).expect("key claimed twice?");

        if anyone_waiting {
            let database_key = DatabaseKeyIndex::new(self.sync_table.ingredient, self.key_index);
            self.zalsa.runtime().unblock_queries_blocked_on(
                database_key,
                if thread::panicking() {
                    tracing::info!("Unblocking queries blocked on {database_key:?} after a panick");
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
