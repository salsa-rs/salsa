use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::mem::ManuallyDrop;
use std::sync::Arc;

use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::function::DeletedEntries;
use crate::revision::AtomicRevision;
use crate::table::memo::MemoTable;
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa_local::QueryOrigin;
use crate::{
    key::DatabaseKeyIndex, zalsa::Zalsa, zalsa_local::QueryRevisions, Event, EventKind, Id,
    Revision,
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

    /// Convert from an internal memo (which uses `'static`) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self<'db>(&'db self, memo: ArcMemo<'static, C>) -> ArcMemo<'db, C> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Inserts the memo for the given key; (atomically) overwrites and returns any previously existing memo
    ///
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(super) unsafe fn insert_memo_into_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: ArcMemo<'db, C>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ManuallyDrop<ArcMemo<'db, C>>> {
        let static_memo = unsafe { self.to_static(memo) };
        let old_static_memo = unsafe {
            zalsa
                .memo_table_for(id)
                .insert(memo_ingredient_index, static_memo)
        }?;
        let old_static_memo = ManuallyDrop::into_inner(old_static_memo);
        Some(ManuallyDrop::new(unsafe { self.to_self(old_static_memo) }))
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ArcMemo<'db, C>> {
        let static_memo = zalsa.memo_table_for(id).get(memo_ingredient_index)?;
        unsafe { Some(self.to_self(static_memo)) }
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, BaseInput,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: &mut MemoTable,
        deleted_entries: &DeletedEntries<C>,
        memo_ingredient_index: MemoIngredientIndex,
    ) {
        let map = |memo: ArcMemo<'static, C>| -> ArcMemo<'static, C> {
            match &memo.revisions.origin {
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
                    // Note that we cannot use `Arc::get_mut` here as the use of `ArcSwap` makes it
                    // impossible to get unique access to the interior Arc
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
                        None,
                        memo.verified_at.load(),
                        QueryRevisions {
                            changed_at,
                            durability,
                            origin: origin.clone(),
                            tracked_struct_ids: tracked_struct_ids.clone(),
                            accumulated: accumulated.clone(),
                            accumulated_inputs: accumulated_inputs.clone(),
                        },
                    ))
                }
            }
        };
        // SAFETY: We queue the old value for deletion, delaying its drop until the next revision bump.
        let old = unsafe { table.map_memo(memo_ingredient_index, map) };
        if let Some(old) = old {
            // In case there is a reference to the old memo out there, we have to store it
            // in the deleted entries. This will get cleared when a new revision starts.
            deleted_entries.push(ManuallyDrop::into_inner(old));
        }
    }
}

#[derive(Debug)]
pub(super) struct Memo<V> {
    /// The result of the query, if we decide to memoize it.
    pub(super) value: Option<V>,

    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicRevision,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

// Memo's are stored a lot, make sure their size is doesn't randomly increase.
// #[cfg(test)]
const _: [(); std::mem::size_of::<Memo<std::num::NonZeroUsize>>()] =
    [(); std::mem::size_of::<[usize; 12]>()];

impl<V> Memo<V> {
    pub(super) fn new(value: Option<V>, revision_now: Revision, revisions: QueryRevisions) -> Self {
        Memo {
            value,
            verified_at: AtomicRevision::from(revision_now),
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
        zalsa: &Zalsa,
        db: &dyn crate::Database,
        database_key_index: DatabaseKeyIndex,
    ) {
        for output in self.revisions.origin.outputs() {
            output.mark_validated_output(zalsa, db, database_key_index);
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
