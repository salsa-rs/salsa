use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use crossbeam_utils::atomic::AtomicCell;

use crate::{
    hash::FxDashMap, key::DatabaseKeyIndex, runtime::local_state::QueryRevisions, AsId, Event,
    EventKind, Revision, Runtime,
};

/// The memo map maps from a key of type `K` to the memoized value for that `K`.
/// The memoized value is a `Memo<V>` which contains, in addition to the value `V`,
/// dependency information.
pub(super) struct MemoMap<K: AsId, V> {
    map: FxDashMap<K, ArcSwap<Memo<V>>>,
}

impl<K: AsId, V> Default for MemoMap<K, V> {
    fn default() -> Self {
        Self {
            map: Default::default(),
        }
    }
}

impl<K: AsId, V> MemoMap<K, V> {
    /// Inserts the memo for the given key; (atomically) overwrites any previously existing memo.-
    #[must_use]
    pub(super) fn insert(&self, key: K, memo: Arc<Memo<V>>) -> Option<ArcSwap<Memo<V>>> {
        self.map.insert(key, ArcSwap::from(memo))
    }

    /// Removes any existing memo for the given key.
    #[must_use]
    pub(super) fn remove(&self, key: K) -> Option<ArcSwap<Memo<V>>> {
        self.map.remove(&key).map(|o| o.1)
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get(&self, key: K) -> Option<Guard<Arc<Memo<V>>>> {
        self.map.get(&key).map(|v| v.load())
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, BaseInput,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict(&self, key: K) {
        use crate::runtime::local_state::QueryOrigin;
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
                        None::<V>,
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
    pub(super) fn check_durability(&self, runtime: &Runtime) -> bool {
        let last_changed = runtime.last_changed_revision(self.revisions.durability);
        let verified_at = self.verified_at.load();
        log::debug!(
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
        runtime: &crate::Runtime,
        database_key_index: DatabaseKeyIndex,
    ) {
        db.salsa_event(Event {
            runtime_id: runtime.id(),
            kind: EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            },
        });

        self.verified_at.store(runtime.current_revision());

        // Also mark the outputs as verified
        for output in self.revisions.origin.outputs() {
            db.mark_validated_output(database_key_index, output);
        }
    }
}
