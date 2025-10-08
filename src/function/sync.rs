use rustc_hash::FxHashMap;

use crate::key::DatabaseKeyIndex;
use crate::runtime::{BlockResult, ClaimTransferredResult, Running, WaitResult};
use crate::sync::thread::{self};
use crate::sync::Mutex;
use crate::tracing;
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
    Cycle { inner: bool },
    /// Successfully claimed the query.
    Claimed(ClaimGuard<'a>),
}

pub(crate) struct SyncState {
    /// The thread id that is owning this query (actively executing it or iterating it as part of a larger cycle).
    id: SyncOwnerId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: bool,

    is_transfer_target: bool,
    claimed_twice: bool,
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
        zalsa: &'me Zalsa,
        key_index: Id,
        allow_reentry: bool,
    ) -> ClaimResult<'me> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                let id = occupied_entry.get().id;

                let id = match id {
                    SyncOwnerId::Thread(id) => id,
                    SyncOwnerId::Transferred => {
                        let current_id = thread::current().id();
                        let database_key_index = DatabaseKeyIndex::new(self.ingredient, key_index);
                        return match zalsa
                            .runtime()
                            .claim_transferred(database_key_index, allow_reentry)
                        {
                            ClaimTransferredResult::ClaimedBy(other_thread) => {
                                occupied_entry.get_mut().anyone_waiting = true;

                                match other_thread.block(write) {
                                    BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                                    BlockResult::Running(running) => ClaimResult::Running(running),
                                }
                            }
                            ClaimTransferredResult::Reentrant => {
                                let SyncState {
                                    id, claimed_twice, ..
                                } = occupied_entry.into_mut();

                                if *claimed_twice {
                                    return ClaimResult::Cycle { inner: false };
                                }

                                *id = SyncOwnerId::Thread(current_id);
                                *claimed_twice = true;

                                ClaimResult::Claimed(ClaimGuard {
                                    key_index,
                                    zalsa,
                                    sync_table: self,
                                    mode: ReleaseMode::SelfOnly,
                                })
                            }
                            ClaimTransferredResult::Cycle { inner: nested } => {
                                ClaimResult::Cycle { inner: nested }
                            }
                            ClaimTransferredResult::Released => {
                                occupied_entry.insert(SyncState {
                                    id: SyncOwnerId::Thread(thread::current().id()),
                                    anyone_waiting: false,
                                    is_transfer_target: false,
                                    claimed_twice: false,
                                });
                                ClaimResult::Claimed(ClaimGuard {
                                    key_index,
                                    zalsa,
                                    sync_table: self,
                                    mode: ReleaseMode::Default,
                                })
                            }
                        };
                    }
                };

                let &mut SyncState {
                    ref mut anyone_waiting,
                    ..
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
                    BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: SyncOwnerId::Thread(thread::current().id()),
                    anyone_waiting: false,
                    is_transfer_target: false,
                    claimed_twice: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    sync_table: self,
                    mode: ReleaseMode::Default,
                })
            }
        }
    }

    fn make_transfer_target(&self, key_index: Id) -> Option<SyncOwnerId> {
        let mut syncs = self.syncs.lock();
        syncs.get_mut(&key_index).map(|state| {
            state.anyone_waiting = true;
            state.is_transfer_target = true;

            state.id
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SyncOwnerId {
    /// Entry is owned by this thread
    Thread(thread::ThreadId),
    /// Entry has been transferred and is owned by another thread.
    /// The id is known by the `DependencyGraph`.
    Transferred,
}

/// Marks an active 'claim' in the synchronization map. The claim is
/// released when this value is dropped.
#[must_use]
pub struct ClaimGuard<'me> {
    key_index: Id,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
    mode: ReleaseMode,
}

impl<'me> ClaimGuard<'me> {
    pub(crate) const fn zalsa(&self) -> &'me Zalsa {
        self.zalsa
    }

    pub(crate) const fn database_key_index(&self) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.sync_table.ingredient, self.key_index)
    }

    pub(crate) fn set_release_mode(&mut self, mode: ReleaseMode) {
        self.mode = mode;
    }

    #[inline(always)]
    fn release_default(&self, wait_result: WaitResult) {
        let mut syncs = self.sync_table.syncs.lock();
        let state = syncs.remove(&self.key_index).expect("key claimed twice?");

        self.release(wait_result, state);
    }

    #[inline(always)]
    fn release(&self, wait_result: WaitResult, state: SyncState) {
        let database_key_index = self.database_key_index();
        tracing::debug!("release_and_unblock({database_key_index:?})");

        let SyncState {
            anyone_waiting,
            is_transfer_target,
            claimed_twice,
            ..
        } = state;

        if !anyone_waiting {
            return;
        }

        let runtime = self.zalsa.runtime();

        if claimed_twice {
            runtime.remove_transferred(database_key_index);
        }

        if is_transfer_target {
            tracing::debug!("unblock transferred queries owned by {database_key_index:?}");
            runtime.unblock_transferred_queries(database_key_index, wait_result);
        }

        runtime.unblock_queries_blocked_on(database_key_index, wait_result);
    }

    #[cold]
    fn release_self(&self) {
        tracing::debug!("release_self");
        let mut syncs = self.sync_table.syncs.lock();
        let std::collections::hash_map::Entry::Occupied(mut state) = syncs.entry(self.key_index)
        else {
            panic!("key claimed twice?");
        };

        if state.get().claimed_twice {
            state.get_mut().claimed_twice = false;
            state.get_mut().id = SyncOwnerId::Transferred;
        } else {
            self.release(WaitResult::Completed, state.remove());
        }
    }

    #[cold]
    pub(crate) fn transfer(&self, new_owner: DatabaseKeyIndex) {
        let self_key = self.database_key_index();

        let owner_ingredient = self.zalsa.lookup_ingredient(new_owner.ingredient_index());

        // Get the owning thread of `new_owner`.
        let owner_sync_table = owner_ingredient.sync_table();
        let owner_thread_id = owner_sync_table
            .make_transfer_target(new_owner.key_index())
            .expect("new owner to be a locked query");

        tracing::debug!(
            "Transferring ownership of {self_key:?} to {new_owner:?} ({owner_thread_id:?})"
        );

        let mut syncs = self.sync_table.syncs.lock();

        let runtime = self.zalsa.runtime();
        runtime.transfer_lock(self_key, thread::current().id(), new_owner, owner_thread_id);

        let SyncState {
            anyone_waiting,
            id,
            claimed_twice,
            ..
        } = syncs.get_mut(&self.key_index).expect("key claimed twice?");

        *id = SyncOwnerId::Transferred;
        *claimed_twice = false;
        *anyone_waiting = false;

        tracing::debug!("Transfer ownership completed");
    }
}

impl Drop for ClaimGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        let wait_result = if thread::panicking() {
            WaitResult::Panicked
        } else {
            WaitResult::Completed
        };

        // TODO, what to do if thread panics? Always force release?
        match self.mode {
            ReleaseMode::Default => {
                self.release_default(wait_result);
            }
            _ if matches!(wait_result, WaitResult::Panicked) => {
                tracing::debug!("Release after panicked");
                self.release_default(wait_result);
            }
            ReleaseMode::SelfOnly => {
                self.release_self();
            }
            ReleaseMode::TransferTo(new_owner) => {
                self.transfer(new_owner);
            }
        }
    }
}

impl std::fmt::Debug for SyncTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncTable").finish()
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) enum ReleaseMode {
    /// The default release mode.
    ///
    /// Releases the lock of the current query for claims that are not transferred. Queries who's ownership
    /// were transferred to this query will be transitively unlocked.
    ///
    /// If this lock is owned by another query (because it was transferred), then releasing is a no-op.
    #[default]
    Default,

    SelfOnly,

    /// Transfers the ownership of the lock to the specified query.
    ///
    /// All waiting queries will be awakened so that they can retry and block on the new owner thread.
    /// The new owner thread (or any thread it blocks on) will be able to acquire the lock (reentrant).
    TransferTo(DatabaseKeyIndex),
}

impl std::fmt::Debug for ClaimGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimGuard")
            .field("key_index", &self.key_index)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}
