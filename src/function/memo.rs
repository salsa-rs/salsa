use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use crossbeam::atomic::AtomicCell;

use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::function::lru::LruChoice;
use crate::function::lru::NoLru;
use crate::table::memo::MemoTable;
use crate::zalsa_local::QueryOrigin;
use crate::{
    key::DatabaseKeyIndex, zalsa::Zalsa, zalsa_local::QueryRevisions, Event, EventKind, Id,
    Revision,
};

use super::{Configuration, IngredientImpl};

#[allow(type_alias_bounds)]
pub(super) type ArcMemo<'lt, C: Configuration> =
    Arc<Memo<C::Lru, <C as Configuration>::Output<'lt>>>;

impl<C: Configuration> IngredientImpl<C> {
    /// Memos have to be stored internally using `'static` as the database lifetime.
    /// This (unsafe) function call converts from something tied to self to static.
    /// Values transmuted this way have to be transmuted back to being tied to self
    /// when they are returned to the user.
    unsafe fn to_static<'db>(&'db self, memo: ArcMemo<'db, C>) -> ArcMemo<'static, C> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Convert from an internal memo (which uses `'static``) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self<'db>(&'db self, memo: ArcMemo<'static, C>) -> ArcMemo<'db, C> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Inserts the memo for the given key; (atomically) overwrites any previously existing memo.
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
    pub(super) fn evict_value_from_memo_for(&self, table: &MemoTable) {
        C::Lru::if_enabled(|| {
            table.map_memo::<Memo<C::Lru, C::Output<'_>>>(self.memo_ingredient_index, |memo| {
                match memo.revisions.origin {
                    QueryOrigin::Assigned(_)
                    | QueryOrigin::DerivedUntracked(_)
                    | QueryOrigin::BaseInput => {
                        // Careful: Cannot evict memos whose values were
                        // assigned as output of another query
                        // or those with untracked inputs
                        // as their values cannot be reconstructed.
                        memo
                    }
                    QueryOrigin::Derived(_) => {
                        // QueryRevisions: !Clone to discourage cloning, we need it here though
                        let &QueryRevisions {
                            changed_at,
                            durability,
                            ref origin,
                            ref tracked_struct_ids,
                            ref accumulated,
                            ref accumulated_inputs,
                        } = &memo.revisions;
                        // Re-assemble the memo but with the value set to `None`
                        Arc::new(Memo::new(
                            C::Lru::evicted(),
                            memo.verified_at.load(),
                            QueryRevisions {
                                changed_at,
                                durability,
                                origin: origin.clone(),
                                tracked_struct_ids: tracked_struct_ids.clone(),
                                accumulated: accumulated.clone(),
                                accumulated_inputs: AtomicCell::new(accumulated_inputs.load()),
                            },
                        ))
                    }
                }
            });
        })
    }
}

pub(super) struct Memo<L: LruChoice, V: Send + Sync> {
    /// The result of the query, if we decide to memoize it.
    pub(super) value: L::LruCtor<V>,

    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicCell<Revision>,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

impl<L: LruChoice<LruCtor<V>: Send + Sync + Debug>, V: Send + Sync + Debug> Debug for Memo<L, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memo")
            .field("value", &self.value)
            .field("verified_at", &self.verified_at)
            .field("revisions", &self.revisions)
            .finish()
    }
}

// Memo's are stored a lot, make sure their size is doesn't randomly increase.
// #[cfg(test)]
const _: [(); std::mem::size_of::<Memo<NoLru, std::num::NonZeroUsize>>()] =
    [(); std::mem::size_of::<[usize; 12]>()];

impl<L: LruChoice, V: Send + Sync> Memo<L, V> {
    pub(super) fn new(
        value: L::LruCtor<V>,
        revision_now: Revision,
        revisions: QueryRevisions,
    ) -> Self {
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
        accumulated: InputAccumulatedValues,
    ) {
        db.salsa_event(&|| {
            Event::new(EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            })
        });

        self.verified_at.store(revision_now);
        self.revisions.accumulated_inputs.store(accumulated);
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
        struct TracingDebug<'a, L: LruChoice, V: Send + Sync> {
            memo: &'a Memo<L, V>,
        }

        impl<L: LruChoice, V: Send + Sync> std::fmt::Debug for TracingDebug<'_, L, V> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Memo")
                    .field(
                        "value",
                        if L::is_evicted(&self.memo.value) {
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

impl<L, V> crate::table::memo::Memo for Memo<L, V>
where
    L: LruChoice<LruCtor<V>: Send + Sync + Any> + 'static,
    V: Send + Sync + Any,
{
    fn origin(&self) -> &QueryOrigin {
        &self.revisions.origin
    }
}
