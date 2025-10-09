use rustc_hash::FxHashMap;
use std::collections::hash_map::OccupiedEntry;

use crate::key::DatabaseKeyIndex;
use crate::runtime::{
    BlockOnTransferredOwner, BlockResult, BlockTransferredResult, Running, WaitResult,
};
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
    Cycle {
        /// `true` if this is a cycle with an inner query. For example, if `a` transferred its ownership to
        /// `b`. If the thread claiming `b` tries to claim `a`, then this results in a cycle unless
        /// `REENTRANT` is `true` (in which case it can be claimed).
        inner: bool,
    },
    /// Successfully claimed the query.
    Claimed(ClaimGuard<'a>),
}

pub(crate) struct SyncState {
    /// The thread id that is owning this query (actively executing it or iterating it as part of a larger cycle).
    id: SyncOwnerId,

    /// Set to true if any other queries are blocked,
    /// waiting for this query to complete.
    anyone_waiting: bool,

    /// Whether any other query has transferred its lock ownership to this query.
    /// This is only an optimization so that the expensive unblocking of transferred queries
    /// can be skipped if `false`. This field might be `true` in cases where queries *were* transferred
    /// to this query, but have since then been transferred to another query (in a later iteration).
    is_transfer_target: bool,

    /// Whether this query has been claimed by the query that currently owns it.
    ///
    /// If `a` has been transferred to `b` and the stack for t1 is `b -> a`, then `a` can be claimed
    /// and `claimed_transferred` is set to `true`. However, t2 won't be able to claim `a` because
    /// it doesn't own `b`.
    claimed_transferred: bool,
}

impl SyncTable {
    pub(crate) fn new(ingredient: IngredientIndex) -> Self {
        Self {
            syncs: Default::default(),
            ingredient,
        }
    }

    /// Claims the given key index, or blocks if it is running on another thread.
    ///
    /// `REENTRANT` controls whether a query that transferred its ownership to another query for which
    /// this thread currently holds the lock for can be claimed. For example, if `a` transferred its ownership
    /// to `b`, and this thread holds the lock for `b`, then this thread can also claim `a` but only if `REENTRANT` is `true`.
    #[inline]
    pub(crate) fn try_claim<'me, const REENTRANT: bool>(
        &'me self,
        zalsa: &'me Zalsa,
        key_index: Id,
    ) -> ClaimResult<'me> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let id = match occupied_entry.get().id {
                    SyncOwnerId::Thread(id) => id,
                    SyncOwnerId::Transferred => {
                        return match self.try_claim_transferred::<REENTRANT>(zalsa, occupied_entry)
                        {
                            Ok(claimed) => claimed,
                            Err(other_thread) => match other_thread.block(write) {
                                BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                                BlockResult::Running(running) => ClaimResult::Running(running),
                            },
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
                    BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(SyncState {
                    id: SyncOwnerId::Thread(thread::current().id()),
                    anyone_waiting: false,
                    is_transfer_target: false,
                    claimed_transferred: false,
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

    #[cold]
    #[inline(never)]
    fn try_claim_transferred<'me, const REENTRANT: bool>(
        &'me self,
        zalsa: &'me Zalsa,
        mut entry: OccupiedEntry<Id, SyncState>,
    ) -> Result<ClaimResult<'me>, Box<BlockOnTransferredOwner<'me>>> {
        let key_index = *entry.key();
        let database_key_index = DatabaseKeyIndex::new(self.ingredient, key_index);
        let thread_id = thread::current().id();

        match zalsa
            .runtime()
            .block_transferred(database_key_index, thread_id)
        {
            BlockTransferredResult::ImTheOwner if REENTRANT => {
                let SyncState {
                    id,
                    claimed_transferred,
                    ..
                } = entry.into_mut();
                debug_assert!(!*claimed_transferred);

                *id = SyncOwnerId::Thread(thread_id);
                *claimed_transferred = true;

                Ok(ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    sync_table: self,
                    mode: ReleaseMode::SelfOnly,
                }))
            }
            BlockTransferredResult::ImTheOwner => Ok(ClaimResult::Cycle { inner: true }),
            BlockTransferredResult::OwnedBy(other_thread) => {
                entry.get_mut().anyone_waiting = true;
                Err(other_thread)
            }
            BlockTransferredResult::Released => {
                entry.insert(SyncState {
                    id: SyncOwnerId::Thread(thread_id),
                    anyone_waiting: false,
                    is_transfer_target: false,
                    claimed_transferred: false,
                });
                Ok(ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    sync_table: self,
                    mode: ReleaseMode::Default,
                }))
            }
        }
    }

    /// Makes `key_index` an owner of a transferred query.
    ///
    /// Returns the `SyncOwnerId` of the thread that currently owns this query.
    ///
    /// Note: The result of this method will immediately become stale unless the thread owning `key_index`
    /// is currently blocked on this thread (claiming `key_index` from this thread results in a cycle).
    fn make_owner_of(&self, key_index: Id) -> Option<SyncOwnerId> {
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
    /// Query is owned by this thread
    Thread(thread::ThreadId),

    /// The query's lock ownership has been transferred to another query.
    /// E.g. if `a` transfers its ownership to `b`, then only the thread in the critical path
    /// to complete b` can claim `a` (in most instances, only the thread owning `b` can claim `a`).
    ///
    /// The thread owning `a` is stored in the `DependencyGraph`.
    ///
    /// A query can be marked as `Transferred` even if it has since then been released by the owning query.
    /// In that case, the query is effectively unclaimed and the `Transferred` state is stale. The reason
    /// for this is that it avoids the need for locking each sync table when releasing the transferred queries.
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

        let SyncState {
            anyone_waiting,
            is_transfer_target,
            claimed_transferred: claimed_twice,
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
            runtime.unblock_transferred_queries(database_key_index, wait_result);
        }

        runtime.unblock_queries_blocked_on(database_key_index, wait_result);
    }

    #[cold]
    #[inline(never)]
    fn release_self(&self) {
        let mut syncs = self.sync_table.syncs.lock();
        let std::collections::hash_map::Entry::Occupied(mut state) = syncs.entry(self.key_index)
        else {
            panic!("key claimed twice?");
        };

        if state.get().claimed_transferred {
            state.get_mut().claimed_transferred = false;
            state.get_mut().id = SyncOwnerId::Transferred;
        } else {
            self.release(WaitResult::Completed, state.remove());
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn transfer(&self, new_owner: DatabaseKeyIndex) {
        tracing::info!("transfer");
        let self_key = self.database_key_index();

        let owner_ingredient = self.zalsa.lookup_ingredient(new_owner.ingredient_index());

        // Get the owning thread of `new_owner`.
        let owner_sync_table = owner_ingredient.sync_table();
        let owner_thread_id = owner_sync_table
            .make_owner_of(new_owner.key_index())
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
            claimed_transferred: claimed_twice,
            ..
        } = syncs.get_mut(&self.key_index).expect("key claimed twice?");

        *id = SyncOwnerId::Transferred;
        *claimed_twice = false;
        *anyone_waiting = false;
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

        match self.mode {
            ReleaseMode::Default => {
                self.release_default(wait_result);
            }
            _ if matches!(wait_result, WaitResult::Panicked) => {
                tracing::debug!("Releasing `ClaimGuard` after panic");
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

/// Controls how the lock is released when the `ClaimGuard` is dropped.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) enum ReleaseMode {
    /// The default release mode.
    ///
    /// Releases the query for which this claim guard holds the lock and any queries that have
    /// transferred ownership to this query.
    #[default]
    Default,

    /// Only releases the lock for this query. Any query that has transferred ownership to this query
    /// will remain locked.
    ///
    /// If this thread panics, the query will be released as normal (default mode).
    SelfOnly,

    /// Transfers the ownership of the lock to the specified query.
    ///
    /// The query will remain locked except the query that's currently blocking this query from completing
    /// (to avoid deadlocks).
    ///
    /// If this thread panics, the query will be released as normal (default mode).
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
