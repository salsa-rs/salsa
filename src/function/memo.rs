use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use crossbeam::atomic::AtomicCell;

use crate::zalsa_local::QueryOrigin;
use crate::{
    cycle::{Cycle, CycleRecoveryStrategy},
    key::DatabaseKeyIndex,
    zalsa::Zalsa,
    zalsa_local::QueryRevisions,
    Event, EventKind, Id, Revision,
};

use super::{Configuration, IngredientImpl};

#[allow(type_alias_bounds)]
pub(super) type ArcMemo<'lt, C: Configuration> = Arc<Memo<<C as Configuration>::Output<'lt>>>;

impl<C: Configuration> IngredientImpl<C> {
    /// Memos have to be stored internally using `'static` as the database lifetime.
    /// This (unsafe) function call converts from something tied to self to static.
    /// Values transmuted this way have to be transmuted back to being tied to self
    /// when they are returned to the user.
    unsafe fn to_static<'db>(&'db self, memo: ArcMemo<'db, C>) -> ArcMemo<'static, C> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Convert from an internal memo (which uses statis) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self<'db>(&'db self, memo: ArcMemo<'static, C>) -> ArcMemo<'db, C> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Inserts the memo for the given key; (atomically) overwrites any previously existing memo.-
    pub(super) fn insert_memo_into_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: ArcMemo<'db, C>,
    ) -> Option<ArcMemo<'db, C>> {
        let static_memo = unsafe { self.to_static(memo) };
        let old_static_memo = zalsa
            .memo_table_for(id)
            .insert(self.memo_ingredient_index, static_memo)?;
        unsafe { Some(self.to_self(old_static_memo)) }
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
    ) -> Option<ArcMemo<'db, C>> {
        let static_memo = zalsa.memo_table_for(id).get(self.memo_ingredient_index)?;
        unsafe { Some(self.to_self(static_memo)) }
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, BaseInput,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for<'db>(&'db self, zalsa: &'db Zalsa, id: Id) {
        let Some(memo) = self.get_memo_from_table_for(zalsa, id) else {
            return;
        };

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

                self.insert_memo_into_table_for(zalsa, id, memo_evicted);
            }
        }
    }

    pub(super) fn initial_value<'db>(&'db self, db: &'db C::DbView) -> Option<C::Output<'db>> {
        match C::CYCLE_STRATEGY {
            CycleRecoveryStrategy::Fixpoint => Some(C::cycle_initial(db)),
            CycleRecoveryStrategy::Panic => None,
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

    /// Cycle, if this result was created in cycle iteration
    pub(super) cycle: Option<Cycle>,
}

impl<V> Memo<V> {
    pub(super) fn new(value: Option<V>, revision_now: Revision, revisions: QueryRevisions) -> Self {
        Memo {
            value,
            verified_at: AtomicCell::new(revision_now),
            revisions,
            cycle: None,
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

    pub(super) fn tracing_debug(&self) -> impl std::fmt::Debug + '_ {
        struct TracingDebug<'a, T> {
            memo: &'a Memo<T>,
        }

        impl<T> std::fmt::Debug for TracingDebug<'_, T> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Memo")
                    .field(
                        "value",
                        if self.memo.value.is_some() {
                            &"Some(<value>)"
                        } else {
                            &"None"
                        },
                    )
                    .field("verified_at", &self.memo.verified_at)
                    .field("revisions", &self.memo.revisions)
                    .finish()
            }
        }

        TracingDebug { memo: self }
    }
}

impl<V: Debug + Send + Sync + Any> crate::table::memo::Memo for Memo<V> {
    fn origin(&self) -> &QueryOrigin {
        &self.revisions.origin
    }
}
