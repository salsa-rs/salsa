use rustc_hash::FxHashMap;
use std::collections::hash_map::OccupiedEntry;

use crate::key::DatabaseKeyIndex;
use crate::plumbing::ZalsaLocal;
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

pub(crate) enum ClaimResult<'a, Guard = ClaimGuard<'a>> {
    /// Successfully claimed the query.
    Claimed(Guard),
    /// Can't claim the query because it is running on an other thread.
    Running(Running<'a>),
    /// Claiming the query results in a cycle.
    Cycle {
        /// `true` if this is a cycle with an inner query. For example, if `a` transferred its ownership to
        /// `b`. If the thread claiming `b` tries to claim `a`, then this results in a cycle except when calling
        /// [`SyncTable::try_claim`] with [`Reentrant::Allow`].
        inner: bool,
    },
}

pub(crate) struct SyncState {
    /// The thread id that currently owns this query (actively executing it or iterating it as part of a larger cycle).
    id: SyncOwner,

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
    /// and `claimed_twice` is set to `true`. However, t2 won't be able to claim `a` because
    /// it doesn't own `b`.
    claimed_twice: bool,
}

impl SyncTable {
    pub(crate) fn new(ingredient: IngredientIndex) -> Self {
        Self {
            syncs: Default::default(),
            ingredient,
        }
    }

    /// Claims the given key index, or blocks if it is running on another thread.
    pub(crate) fn try_claim<'me>(
        &'me self,
        zalsa: &'me Zalsa,
        zalsa_local: &'me ZalsaLocal,
        key_index: Id,
        reentrant: Reentrancy,
    ) -> ClaimResult<'me> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let id = match occupied_entry.get().id {
                    SyncOwner::Thread(id) => id,
                    SyncOwner::Transferred => {
                        return match self.try_claim_transferred(
                            zalsa,
                            zalsa_local,
                            occupied_entry,
                            reentrant,
                        ) {
                            Ok(claimed) => claimed,
                            Err(other_thread) => match other_thread.block(write) {
                                BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                                BlockResult::Running(running) => ClaimResult::Running(running),
                            },
                        }
                    }
                };

                let SyncState { anyone_waiting, .. } = occupied_entry.into_mut();

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
                    id: SyncOwner::Thread(thread::current().id()),
                    anyone_waiting: false,
                    is_transfer_target: false,
                    claimed_twice: false,
                });
                ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    zalsa_local,
                    sync_table: self,
                    mode: ReleaseMode::Default,
                })
            }
        }
    }

    /// Claims the given key index, or blocks if it is running on another thread.
    pub(crate) fn peek_claim<'me>(
        &'me self,
        zalsa: &'me Zalsa,
        key_index: Id,
        reentrant: Reentrancy,
    ) -> ClaimResult<'me, ()> {
        let mut write = self.syncs.lock();
        match write.entry(key_index) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let id = match occupied_entry.get().id {
                    SyncOwner::Thread(id) => id,
                    SyncOwner::Transferred => {
                        return match self.peek_claim_transferred(zalsa, occupied_entry, reentrant) {
                            Ok(claimed) => claimed,
                            Err(other_thread) => match other_thread.block(write) {
                                BlockResult::Cycle => ClaimResult::Cycle { inner: false },
                                BlockResult::Running(running) => ClaimResult::Running(running),
                            },
                        }
                    }
                };

                let SyncState { anyone_waiting, .. } = occupied_entry.into_mut();

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
            std::collections::hash_map::Entry::Vacant(_) => ClaimResult::Claimed(()),
        }
    }

    #[cold]
    #[inline(never)]
    fn try_claim_transferred<'me>(
        &'me self,
        zalsa: &'me Zalsa,
        zalsa_local: &'me ZalsaLocal,
        mut entry: OccupiedEntry<Id, SyncState>,
        reentrant: Reentrancy,
    ) -> Result<ClaimResult<'me>, Box<BlockOnTransferredOwner<'me>>> {
        let key_index = *entry.key();
        let database_key_index = DatabaseKeyIndex::new(self.ingredient, key_index);
        let thread_id = thread::current().id();

        match zalsa
            .runtime()
            .block_transferred(database_key_index, thread_id)
        {
            BlockTransferredResult::ImTheOwner if reentrant.is_allow() => {
                let SyncState {
                    id, claimed_twice, ..
                } = entry.into_mut();
                debug_assert!(!*claimed_twice);

                *id = SyncOwner::Thread(thread_id);
                *claimed_twice = true;

                Ok(ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    zalsa_local,
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
                    id: SyncOwner::Thread(thread_id),
                    anyone_waiting: false,
                    is_transfer_target: false,
                    claimed_twice: false,
                });
                Ok(ClaimResult::Claimed(ClaimGuard {
                    key_index,
                    zalsa,
                    zalsa_local,
                    sync_table: self,
                    mode: ReleaseMode::Default,
                }))
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn peek_claim_transferred<'me>(
        &'me self,
        zalsa: &'me Zalsa,
        mut entry: OccupiedEntry<Id, SyncState>,
        reentrant: Reentrancy,
    ) -> Result<ClaimResult<'me, ()>, Box<BlockOnTransferredOwner<'me>>> {
        let key_index = *entry.key();
        let database_key_index = DatabaseKeyIndex::new(self.ingredient, key_index);
        let thread_id = thread::current().id();

        match zalsa
            .runtime()
            .block_transferred(database_key_index, thread_id)
        {
            BlockTransferredResult::ImTheOwner if reentrant.is_allow() => {
                Ok(ClaimResult::Claimed(()))
            }
            BlockTransferredResult::ImTheOwner => Ok(ClaimResult::Cycle { inner: true }),
            BlockTransferredResult::OwnedBy(other_thread) => {
                entry.get_mut().anyone_waiting = true;
                Err(other_thread)
            }
            BlockTransferredResult::Released => Ok(ClaimResult::Claimed(())),
        }
    }

    /// Marks `key_index` as a transfer target.
    ///
    /// Returns the `SyncOwnerId` of the thread that currently owns this query.
    ///
    /// Note: The result of this method will immediately become stale unless the thread owning `key_index`
    /// is currently blocked on this thread (claiming `key_index` from this thread results in a cycle).
    pub(super) fn mark_as_transfer_target(&self, key_index: Id) -> Option<SyncOwner> {
        let mut syncs = self.syncs.lock();
        syncs.get_mut(&key_index).map(|state| {
            // We set `anyone_waiting` to true because it is used in `ClaimGuard::release`
            // to exit early if the query doesn't need to release any locks.
            // However, there are now dependent queries that need to be released, that's why we set `anyone_waiting` to true,
            // so that `ClaimGuard::release` no longer exits early.
            state.anyone_waiting = true;
            state.is_transfer_target = true;

            state.id
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub enum SyncOwner {
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
pub(crate) struct ClaimGuard<'me> {
    key_index: Id,
    zalsa: &'me Zalsa,
    sync_table: &'me SyncTable,
    mode: ReleaseMode,
    zalsa_local: &'me ZalsaLocal,
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

    #[cold]
    #[inline(never)]
    fn release_panicking(&self) {
        let mut syncs = self.sync_table.syncs.lock();
        let state = syncs.remove(&self.key_index).expect("key claimed twice?");
        tracing::debug!(
            "Release claim on {:?} due to panic",
            self.database_key_index()
        );
        self.release(state, WaitResult::Panicked);
    }

    #[cold]
    #[inline(never)]
    fn release_cancelled(&self) {
        let mut syncs = self.sync_table.syncs.lock();
        let state = syncs.remove(&self.key_index).expect("key claimed twice?");
        tracing::debug!(
            "Release claim on {:?} due to cancellation",
            self.database_key_index()
        );
        self.release(state, WaitResult::Cancelled);
    }

    #[inline(always)]
    fn release(&self, state: SyncState, wait_result: WaitResult) {
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
        let database_key_index = self.database_key_index();

        if claimed_twice {
            runtime.undo_transfer_lock(database_key_index);
        }

        runtime.unblock_queries_blocked_on(database_key_index, wait_result);

        if is_transfer_target {
            runtime.unblock_transferred_queries_owned_by(database_key_index, wait_result);
        }
    }

    #[cold]
    #[inline(never)]
    fn release_self(&self) {
        let mut syncs = self.sync_table.syncs.lock();
        let std::collections::hash_map::Entry::Occupied(mut state) = syncs.entry(self.key_index)
        else {
            panic!("key should only be claimed/released once");
        };

        if state.get().claimed_twice {
            state.get_mut().claimed_twice = false;
            state.get_mut().id = SyncOwner::Transferred;
        } else {
            self.release(state.remove(), WaitResult::Completed);
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn transfer(&self, new_owner: DatabaseKeyIndex) -> bool {
        let owner_ingredient = self.zalsa.lookup_ingredient(new_owner.ingredient_index());

        // Get the owning thread of `new_owner`.
        // The thread id is guaranteed to not be stale because `new_owner` must be blocked on `self_key`
        // or `transfer_lock` will panic (at least in debug builds).
        let Some(new_owner_thread_id) =
            owner_ingredient.mark_as_transfer_target(new_owner.key_index())
        else {
            self.release(
                self.sync_table
                    .syncs
                    .lock()
                    .remove(&self.key_index)
                    .expect("key should only be claimed/released once"),
                WaitResult::Panicked,
            );

            panic!("new owner to be a locked query")
        };

        let mut syncs = self.sync_table.syncs.lock();

        let self_key = self.database_key_index();
        tracing::debug!(
            "Transferring lock ownership of {self_key:?} to {new_owner:?} ({new_owner_thread_id:?})"
        );

        let SyncState {
            id, claimed_twice, ..
        } = syncs
            .get_mut(&self.key_index)
            .expect("key should only be claimed/released once");

        *id = SyncOwner::Transferred;
        *claimed_twice = false;

        self.zalsa
            .runtime()
            .transfer_lock(self_key, new_owner, new_owner_thread_id, syncs)
    }

    /// Drops the claim on the memo.
    ///
    /// Returns `true` if the lock was transferred to another query and
    /// this thread blocked waiting for the new owner's lock to be released.
    /// In that case, any computed memo need to be refetched because they may have
    /// changed since `drop` was called.
    pub(crate) fn drop(mut self) -> bool {
        let refetch = self.drop_impl();
        std::mem::forget(self);
        refetch
    }

    fn drop_impl(&mut self) -> bool {
        match self.mode {
            ReleaseMode::Default => {
                let mut syncs = self.sync_table.syncs.lock();
                let state = syncs
                    .remove(&self.key_index)
                    .expect("key should only be claimed/released once");

                self.release(state, WaitResult::Completed);
                false
            }
            ReleaseMode::SelfOnly => {
                self.release_self();
                false
            }
            ReleaseMode::TransferTo(new_owner) => self.transfer(new_owner),
        }
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            if self.zalsa_local.is_cancelled() {
                self.release_cancelled();
            } else {
                self.release_panicking();
            }
            return;
        }

        self.drop_impl();
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
    /// The query will remain locked and only the thread owning the transfer target will be resumed.
    ///
    /// The transfer target must be a query that's blocked on this query to guarantee that the transfer target doesn't complete
    /// before the transfer is finished (which would leave this query locked forever).
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

/// Controls whether this thread can claim a query that transferred its ownership to a query
/// this thread currently holds the lock for.
///
/// For example: if query `a` transferred its ownership to query `b`, and this thread holds
/// the lock for `b`, then this thread can also claim `a` — but only when using [`Self::Allow`].
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Reentrancy {
    /// Allow `try_claim` to reclaim a query's that transferred its ownership to a query
    /// hold by this thread.
    Allow,

    /// Only allow claiming queries that haven't been claimed by any thread.
    Deny,
}

impl Reentrancy {
    const fn is_allow(self) -> bool {
        matches!(self, Reentrancy::Allow)
    }
}
