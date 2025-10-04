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

    fn make_transfer_target(&self, key_index: Id, zalsa: &Zalsa) -> Option<ThreadId> {
        let mut read = self.syncs.lock();
        read.get_mut(&key_index).map(|state| {
            state.anyone_waiting = true;
            state.is_transfer_target = true;

            match state.id {
                OwnerId::Thread(thread_id) => thread_id,
                OwnerId::Transferred => zalsa
                    .runtime()
                    .resolved_transferred_thread_id(DatabaseKeyIndex::new(
                        self.ingredient,
                        key_index,
                    ))
                    .unwrap(),
            }
        })
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
                    OwnerId::Thread(id) => id,
                    OwnerId::Transferred => {
                        let current_id = thread::current().id();
                        let database_key_index = DatabaseKeyIndex::new(self.ingredient, key_index);
                        match zalsa
                            .runtime()
                            .block_on_transferred(database_key_index, current_id)
                        {
                            Ok((current_owner, owning_thread_id)) => {
                                let SyncState { id, .. } = occupied_entry.into_mut();

                                return if !allow_reentry {
                                    tracing::debug!("Claiming {database_key_index:?} results in a cycle because re-entrant lock is not allowed");
                                    ClaimResult::Cycle(true)
                                } else {
                                    tracing::debug!("Reentrant lock {database_key_index:?}");
                                    *id = OwnerId::Thread(current_id);

                                    zalsa.runtime().remove_transferred(database_key_index);

                                    if owning_thread_id != current_id {
                                        zalsa.runtime().unblock_queries_blocked_on(
                                            database_key_index,
                                            WaitResult::Completed,
                                        );
                                        zalsa.runtime().resume_transferred_queries(
                                            database_key_index,
                                            WaitResult::Completed,
                                        );
                                    }

                                    ClaimResult::Claimed(ClaimGuard {
                                        key_index,
                                        zalsa,
                                        sync_table: self,
                                        mode: ReleaseMode::TransferTo(current_owner),
                                    })
                                };
                            }
                            // Lock is owned by another thread, wait for it to be released.
                            Err(Some(thread_id)) => {
                                tracing::debug!("Waiting for transfered lock {database_key_index:?} to be released by thread {thread_id:?}");
                                thread_id
                            }
                            // Lock was transferred but is no more. Replace the entry.
                            Err(None) => {
                                tracing::debug!(
                                    "Claiming previously transferred lock {database_key_index:?}"
                                );

                                // Lock was transferred but it has since then been released.
                                occupied_entry.insert(SyncState {
                                    id: OwnerId::Thread(thread::current().id()),
                                    anyone_waiting: false,
                                    is_transfer_target: false,
                                });
                                return ClaimResult::Claimed(ClaimGuard {
                                    key_index,
                                    zalsa,
                                    sync_table: self,
                                    mode: ReleaseMode::Default,
                                });
                            }
                        }
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
                    mode: ReleaseMode::Default,
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
    const fn is_thread(&self) -> bool {
        matches!(self, OwnerId::Thread(_))
    }
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

    fn release_default(&self) {
        let mut syncs = self.sync_table.syncs.lock();
        let state = syncs.remove(&self.key_index).expect("key claimed twice?");

        let database_key_index = self.database_key_index();
        tracing::debug!("release_and_unblock({database_key_index:?})");

        let wait_result = if thread::panicking() {
            tracing::info!("Unblocking queries blocked on {database_key_index:?} after a panick");
            WaitResult::Panicked
        } else {
            WaitResult::Completed
        };

        let SyncState {
            anyone_waiting,
            is_transfer_target,
            ..
        } = state;

        if !anyone_waiting {
            return;
        }

        let runtime = self.zalsa.runtime();
        runtime.unblock_queries_blocked_on(database_key_index, wait_result);

        if is_transfer_target {
            tracing::debug!("unblock transferred queries owned by {database_key_index:?}");
            runtime.unblock_transferred_queries(database_key_index, wait_result);
        }
    }

    #[cold]
    pub(crate) fn transfer(&self, new_owner: DatabaseKeyIndex) {
        let self_key = self.database_key_index();

        let owner_ingredient = self.zalsa.lookup_ingredient(new_owner.ingredient_index());

        // Get the owning thread of `new_owner`.
        let owner_sync_table = owner_ingredient.sync_table();
        let owner_thread_id = owner_sync_table
            .make_transfer_target(new_owner.key_index(), self.zalsa)
            .expect("new owner to be a locked query");

        tracing::debug!(
            "Transferring ownership of {self_key:?} to {new_owner:?} ({owner_thread_id:?})"
        );

        let mut syncs = self.sync_table.syncs.lock();

        self.zalsa.runtime().transfer_lock(
            self_key,
            thread::current().id(),
            new_owner,
            owner_thread_id,
        );

        let SyncState {
            anyone_waiting, id, ..
        } = syncs.get_mut(&self.key_index).expect("key claimed twice?");

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
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        // TODO, what to do if thread panics? Always force release?
        match self.mode {
            ReleaseMode::Default => {
                self.release_default();
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
