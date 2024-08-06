use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use crossbeam::atomic::AtomicCell;

use crate::{
    hash::FxDashMap, key::DatabaseKeyIndex, zalsa::Zalsa, zalsa_local::QueryRevisions, Event,
    EventKind, Id, Revision,
};

use super::Configuration;

/// The memo map maps from a key of type `K` to the memoized value for that `K`.
/// The memoized value is a `Memo<V>` which contains, in addition to the value `V`,
/// dependency information.
pub(super) struct MemoMap<C: Configuration> {
    map: FxDashMap<Id, ArcMemo<'static, C>>,
}

#[allow(type_alias_bounds)]
type ArcMemo<'lt, C: Configuration> = ArcSwap<Memo<<C as Configuration>::Output<'lt>>>;

impl<C: Configuration> Default for MemoMap<C> {
    fn default() -> Self {
        Self {
            map: Default::default(),
        }
    }
}

impl<C: Configuration> MemoMap<C> {
    /// Memos have to be stored internally using `'static` as the database lifetime.
    /// This (unsafe) function call converts from something tied to self to static.
    /// Values transmuted this way have to be transmuted back to being tied to self
    /// when they are returned to the user.
    unsafe fn to_static<'db>(&'db self, value: ArcMemo<'db, C>) -> ArcMemo<'static, C> {
        unsafe { std::mem::transmute(value) }
    }

    /// Convert from an internal memo (which uses statis) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self<'db>(&'db self, value: ArcMemo<'static, C>) -> ArcMemo<'db, C> {
        unsafe { std::mem::transmute(value) }
    }
    /// Inserts the memo for the given key; (atomically) overwrites any previously existing memo.-
    #[must_use]
    pub(super) fn insert<'db>(
        &'db self,
        key: Id,
        memo: Arc<Memo<C::Output<'db>>>,
    ) -> Option<ArcSwap<Memo<C::Output<'db>>>> {
        unsafe {
            let value = ArcSwap::from(memo);
            let old_value = self.map.insert(key, self.to_static(value))?;
            Some(self.to_self(old_value))
        }
    }

    /// Removes any existing memo for the given key.
    #[must_use]
    pub(super) fn remove(&self, key: Id) -> Option<ArcSwap<Memo<C::Output<'_>>>> {
        unsafe { self.map.remove(&key).map(|o| self.to_self(o.1)) }
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get<'db>(&self, key: Id) -> Option<Guard<Arc<Memo<C::Output<'db>>>>> {
        self.map.get(&key).map(|v| unsafe {
            std::mem::transmute::<
                Guard<Arc<Memo<C::Output<'static>>>>,
                Guard<Arc<Memo<C::Output<'db>>>>,
            >(v.load())
        })
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, BaseInput,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict(&self, key: Id) {
        use crate::zalsa_local::QueryOrigin;
        use dashmap::mapref::entry::Entry::*;

        if let Occupied(entry) = self.map.entry(key) {
            let memo = entry.get().load();
            match memo.revisions.origin {
                QueryOrigin::Assigned(_)
                | QueryOrigin::DerivedUntracked(_)
                | QueryOrigin::BaseInput => {
                    // Careful: Cannot evict memos whose values were
                    // assigned as output of another query
                    // or those with untracked inputs
                    // as their values cannot be reconstructed.
                }

                QueryOrigin::Derived(_) => {
                    let memo_evicted = Arc::new(Memo::new(
                        None::<C::Output<'_>>,
                        memo.verified_at.load(),
                        memo.revisions.clone(),
                    ));

                    entry.get().store(memo_evicted);
                }
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Memo<V> {
    /// The result of the query, if we decide to memoize it.
    pub(super) value: Option<V>,

    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicCell<Revision>,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

impl<V> Memo<V> {
    pub(super) fn new(value: Option<V>, revision_now: Revision, revisions: QueryRevisions) -> Self {
        Memo {
            value,
            verified_at: AtomicCell::new(revision_now),
            revisions,
        }
    }
    /// True if this memo is known not to have changed based on its durability.
    pub(super) fn check_durability(&self, zalsa: &Zalsa) -> bool {
        let last_changed = zalsa.last_changed_revision(self.revisions.durability);
        let verified_at = self.verified_at.load();
        tracing::debug!(
            "check_durability(last_changed={:?} <= verified_at={:?}) = {:?}",
            last_changed,
            self.verified_at,
            last_changed <= verified_at,
        );
        last_changed <= verified_at
    }

    /// Mark memo as having been verified in the `revision_now`, which should
    /// be the current revision.
    pub(super) fn mark_as_verified(
        &self,
        db: &dyn crate::Database,
        revision_now: Revision,
        database_key_index: DatabaseKeyIndex,
    ) {
        db.salsa_event(&|| Event {
            thread_id: std::thread::current().id(),
            kind: EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            },
        });

        self.verified_at.store(revision_now);
    }

    pub(super) fn mark_outputs_as_verified(
        &self,
        db: &dyn crate::Database,
        database_key_index: DatabaseKeyIndex,
    ) {
        for output in self.revisions.origin.outputs() {
            output.mark_validated_output(db, database_key_index);
        }
    }
}
