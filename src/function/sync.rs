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
    Cycle(bool),
    /// Successfully claimed the query.
    Claimed(ClaimGuard<'a>),
}

pub(crate) struct SyncState {
    /// The thread id that is owning this query (actively executing it or iterating it as part of a larger cycle).
    id: OwnerId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: bool,

    is_transfer_target: bool,
}

impl SyncTable {
    pub(crate) fn new(ingredient: IngredientIndex) -> Self {
        Self {
            syncs: Default::default(),
            ingredient,
        }
    }

    fn make_transfer_target(&self, key_index: Id) -> Option<ThreadId> {
        let mut read = self.syncs.lock();
        read.get_mut(&key_index).map(|state| {
            state.anyone_waiting = true;
            state.is_transfer_target = true;

            match state.id {
                OwnerId::Thread(thread_id) => thread_id,
                OwnerId::Transferred => {
                    panic!("Can't transfer ownership to a query that has been transferred")
                }
            }
        })
    }

    fn remove_from_map_and_unblock_queries(&self, zalsa: &Zalsa, key_index: Id) {
        let mut syncs = self.syncs.lock();

        let SyncState {
            anyone_waiting,
            is_transfer_target,
            ..
        } = syncs.remove(&key_index).expect("key claimed twice?");

        // if !anyone_waiting {
        //     return;
        // }

        let database_key = DatabaseKeyIndex::new(self.ingredient, key_index);
        let wait_result = if thread::panicking() {
            tracing::info!("Unblocking queries blocked on {database_key:?} after a panick");
            WaitResult::Panicked
        } else {
            WaitResult::Completed
        };

        zalsa
            .runtime()
            .unblock_queries_blocked_on(database_key, wait_result);

        // if !is_transfer_target {
        //     return;
        // }

        let transferred_dependents = zalsa.runtime().take_transferred_dependents(database_key);

        drop(syncs);

        for dependent in transferred_dependents {
            let ingredient = zalsa.lookup_ingredient(dependent.ingredient_index());
            ingredient
                .sync_table()
                .remove_from_map_and_unblock_queries(zalsa, dependent.key_index());
        }
    }

    pub(crate) fn try_claim<'me>(
        &'me self,
        zalsa: &'me Zalsa,
        key_index: Id,
        reentry: bool,
    ) -> ClaimResult<'me> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let &mut SyncState {
                    ref mut id,
                    ref mut anyone_waiting,
                    ref mut is_transfer_target,
                } = occupied_entry.into_mut();

                let id = match id {
                    OwnerId::Thread(id) => *id,
                    OwnerId::Transferred => {
                        match zalsa.runtime().transfered_thread_id(
                            DatabaseKeyIndex::new(self.ingredient, key_index),
                            reentry,
                        ) {
                            Ok(owner_thread_id) => {
                                if reentry {
                                    *id = OwnerId::Thread(owner_thread_id);
                                    *is_transfer_target = false;

                                    return ClaimResult::Claimed(ClaimGuard {
                                        key_index,
                                        zalsa,
                                        sync_table: self,
                                        defused: false,
                                    });
                                } else {
                                    return ClaimResult::Cycle(true);
                                }
                            }
                            Err(thread_id) => thread_id,
                        }
                    }
                };

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
                    BlockResult::Cycle => ClaimResult::Cycle(false),
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: OwnerId::Thread(thread::current().id()),
                    anyone_waiting: false,
                    is_transfer_target: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    sync_table: self,
                    defused: false,
                })
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum OwnerId {
    /// Entry is owned by this thread
    Thread(thread::ThreadId),
    /// Entry has been transferred and is owned by another thread.
    /// The id is known by the `DependencyGraph`.
    Transferred,
}

impl OwnerId {
    const fn is_transferred(&self) -> bool {
        matches!(self, OwnerId::Transferred)
    }
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub struct ClaimGuard<'me> {
    key_index: Id,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
    defused: bool,
}

impl ClaimGuard<'_> {
    pub(crate) fn transfer_to(mut self, new_owner: DatabaseKeyIndex) {
        // TODO: If new_owner is already transferred, redirect to its owner instead.

        let self_key = DatabaseKeyIndex::new(self.sync_table.ingredient, self.key_index);
        tracing::debug!("Transferring ownership of {self_key:?} to {new_owner:?}",);

        let owner_ingredient = self.zalsa.lookup_ingredient(new_owner.ingredient_index());

        // Get the owning thread of `new_owner`.
        let owner_sync_table = owner_ingredient.sync_table();
        let owner_thread_id = owner_sync_table
            .make_transfer_target(new_owner.key_index())
            .expect("new owner to be a locked query");

        let mut syncs = self.sync_table.syncs.lock();

        // FIXME: We need to update the sync tables here? No we don't, they're still transferred.
        self.zalsa
            .runtime()
            .transfer_lock(self_key, new_owner, owner_thread_id);

        tracing::debug!("Acquired lock on syncs");

        let SyncState {
            anyone_waiting, id, ..
        } = syncs.get_mut(&self.key_index).expect("key claimed twice?");

        // Transfer ownership
        *id = OwnerId::Transferred;

        // TODO: Do we need to wake up any threads that are awaiting any of the dependents to update the dependency graph -> I think so.
        if *anyone_waiting {
            tracing::debug!(
                "Wake up blocked threads after transferring ownership to {new_owner:?}"
            );
            // Wake up all threads that were waiting on the query to complete so that they'll retry and block on the new owner.
            let database_key = DatabaseKeyIndex::new(self.sync_table.ingredient, self.key_index);
            self.zalsa
                .runtime()
                .unblock_queries_blocked_on(database_key, WaitResult::Completed);
        }

        *anyone_waiting = false;

        tracing::debug!("Transfer ownership completed");

        self.defused = true;
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        if !self.defused {
            self.sync_table
                .remove_from_map_and_unblock_queries(self.zalsa, self.key_index);
        }
    }
}

impl std::fmt::Debug for SyncTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncTable").finish()
    }
}
