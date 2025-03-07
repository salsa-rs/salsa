use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::ptr::NonNull;

use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::key::OutputDependencyIndex;
use crate::plumbing::MemoIngredientMap;
use crate::plumbing::SalsaStructInDb;
use crate::revision::AtomicRevision;
use crate::table::Table;
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa_local::QueryOrigin;
use crate::{
    key::DatabaseKeyIndex, zalsa::Zalsa, zalsa_local::QueryRevisions, Event, EventKind, Id,
    Revision,
};

use super::{Configuration, IngredientImpl};

impl<C: Configuration> IngredientImpl<C> {
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
        memo: NonNull<Memo<C::Output<'db>>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<Memo<C::Output<'db>>>> {
        let static_memo = memo.cast::<Memo<C::Output<'static>>>();
        // SAFETY: We are supplying the correct current revision
        let memos = unsafe { zalsa.table().memos(id, zalsa.current_revision()) };
        // SAFETY: Caller is responsible for upholding the safety invariants
        let old_static_memo = unsafe { memos.insert(memo_ingredient_index, static_memo) }?;
        Some(old_static_memo.cast::<Memo<C::Output<'db>>>())
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        // SAFETY: We are supplying the correct current revision
        let static_memo = unsafe { zalsa.table().memos(id, zalsa.current_revision()) }
            .get(memo_ingredient_index)?;

        // SAFETY: `'db` is the actual lifetime of the memo
        Some(unsafe {
            std::mem::transmute::<&'db Memo<C::Output<'static>>, &'db Memo<C::Output<'db>>>(
                static_memo,
            )
        })
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, BaseInput,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: &mut Table,
        memo_ingredient_indices: &<<C as Configuration>::SalsaStruct<'static> as SalsaStructInDb>::MemoIngredientMap,
        evict: Id,
    ) {
        let memo_ingredient_index = memo_ingredient_indices.get_id_with_table(table, evict);
        let map = |memo: &mut Memo<C::Output<'static>>| {
            match &memo.revisions.origin {
                QueryOrigin::Assigned(_) | QueryOrigin::DerivedUntracked(_) => {
                    // Careful: Cannot evict memos whose values were
                    // assigned as output of another query
                    // or those with untracked inputs
                    // as their values cannot be reconstructed.
                }
                QueryOrigin::Derived(_) => {
                    // Set the memo value to `None`.
                    memo.value = None;
                }
            }
        };

        table.memos_mut(evict).map_memo(memo_ingredient_index, map)
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

    /// Mark memo as having been verified in the `revision_now`, which should
    /// be the current revision.
    pub(super) fn mark_as_verified<Db: ?Sized + crate::Database>(
        &self,
        db: &Db,
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
    fn for_each_output(&self, cb: &mut dyn FnMut(OutputDependencyIndex)) {
        self.revisions.origin.outputs().for_each(cb);
    }
}
