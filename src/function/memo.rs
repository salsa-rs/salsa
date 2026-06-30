use std::fmt::{Debug, Formatter};
use std::ptr::NonNull;

use crate::cycle::{
    CycleHeads, CycleHeadsIterator, IterationStamp, ProvisionalStatus, empty_cycle_heads,
};
use crate::function::{Configuration, IngredientImpl};
use crate::ingredient::WaitForResult;
use crate::key::DatabaseKeyIndex;
use crate::revision::AtomicRevision;
use crate::sync::atomic::Ordering;
use crate::table::memo::MemoTableWithTypesMut;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryOriginRef, QueryRevisions};
use crate::{Event, EventKind, Id, Revision};

impl<C: Configuration> IngredientImpl<C> {
    /// Inserts the memo for the given key; (atomically) overwrites and returns any previously existing memo
    pub(super) fn insert_memo_into_table_for(
        &self,
        zalsa: &Zalsa,
        id: Id,
        memo: NonNull<Memo<C>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<Memo<C>>> {
        zalsa
            .memo_table_for::<C::SalsaStruct<'_>>(id)
            .insert(memo_ingredient_index, memo)
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C>> {
        let memo = zalsa
            .memo_table_for::<C::SalsaStruct<'_>>(id)
            .get(memo_ingredient_index)?;
        // SAFETY: The memo table owns this allocation for at least `'db`.
        Some(unsafe { memo.as_ref() })
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: MemoTableWithTypesMut<'_>,
        memo_ingredient_index: MemoIngredientIndex,
    ) {
        let map = |memo: &mut Memo<C>| {
            if memo.header.can_evict_value() {
                // Set the memo value to `None`.
                memo.value = None;
            }
        };

        table.map_memo(memo_ingredient_index, map)
    }
}

#[derive(Debug)]
pub struct Memo<C: Configuration> {
    /// Configuration-independent state used to validate and manage this memo.
    pub(super) header: MemoHeader,

    /// The result of the query, if we decide to memoize it.
    pub(super) value: Option<C::OutputValue>,
}

#[derive(Debug)]
pub(super) struct MemoHeader {
    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicRevision,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

impl MemoHeader {
    fn new(revision_now: Revision, revisions: QueryRevisions) -> Self {
        debug_assert!(
            !revisions.verified_final.load(Ordering::Relaxed) || revisions.cycle_heads().is_empty(),
            "Memo must be finalized if it has no cycle heads"
        );
        Self {
            verified_at: AtomicRevision::from(revision_now),
            revisions,
        }
    }

    #[inline]
    pub(super) fn origin(&self) -> QueryOriginRef<'_> {
        self.revisions.origin()
    }

    fn can_evict_value(&self) -> bool {
        // Careful: Cannot evict memos whose values were
        // assigned as output of another query
        // or those with untracked inputs
        // as their values cannot be reconstructed.
        matches!(self.origin(), QueryOriginRef::Derived(_))
    }

    /// True if this may be a provisional cycle-iteration result.
    #[inline]
    pub(super) fn may_be_provisional(&self) -> bool {
        // Relaxed is OK here, because `verified_final` is only ever mutated in one direction (from
        // `false` to `true`), and changing it to `true` on memos with cycle heads where it was
        // ever `false` is purely an optimization; if we read an out-of-date `false`, it just means
        // we might go validate it again unnecessarily.
        !self.revisions.verified_final.load(Ordering::Relaxed)
    }

    /// Cycle heads that should be propagated to dependent queries.
    #[inline(always)]
    pub(super) fn cycle_heads(&self) -> &CycleHeads {
        if self.may_be_provisional() {
            self.revisions.cycle_heads()
        } else {
            empty_cycle_heads()
        }
    }

    /// Returns `true` if this memo was part of a cycle in it's last iteration.
    #[inline(always)]
    pub(super) fn was_cycle_participant(&self) -> bool {
        !self.revisions.cycle_heads().is_empty()
    }

    /// Mark memo as having been verified in the `revision_now`, which should
    /// be the current revision.
    /// The caller is responsible to update the memo's `accumulated` state if their accumulated
    /// values have changed since.
    #[inline]
    pub(super) fn mark_as_verified(&self, zalsa: &Zalsa, database_key_index: DatabaseKeyIndex) {
        zalsa.event(&|| {
            Event::new(EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            })
        });

        self.verified_at.store(zalsa.current_revision());
    }

    pub(super) fn mark_outputs_as_verified(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) {
        for output in self.revisions.origin().outputs() {
            output.mark_validated_output(zalsa, database_key_index);
        }
    }

    pub(super) fn tracing_debug(&self, has_value: bool) -> impl std::fmt::Debug + use<'_> {
        struct TracingDebug<'memo> {
            header: &'memo MemoHeader,
            has_value: bool,
        }

        impl std::fmt::Debug for TracingDebug<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Memo")
                    .field(
                        "value",
                        if self.has_value {
                            &"Some(<value>)"
                        } else {
                            &"None"
                        },
                    )
                    .field("verified_at", &self.header.verified_at)
                    .field("revisions", &self.header.revisions)
                    .finish()
            }
        }

        TracingDebug {
            header: self,
            has_value,
        }
    }

    pub(super) fn remove_outputs(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        for stale_output in self.revisions.origin().outputs() {
            stale_output.remove_stale_output(zalsa, executor);
        }

        for (identity, id) in self.revisions.tracked_struct_ids() {
            let key = DatabaseKeyIndex::new(identity.ingredient_index(), *id);
            key.remove_stale_output(zalsa, executor);
        }
    }
}

impl<C: Configuration> Memo<C> {
    pub(super) fn new<'db>(
        value: Option<C::Output<'db>>,
        revision_now: Revision,
        revisions: QueryRevisions,
    ) -> Self {
        Self {
            // SAFETY: Query outputs are erased only while retained in this
            // memo and are rebound to the lifetime of a database borrow.
            value: value.map(|value| unsafe { crate::salsa_value::erase::<C::OutputValue>(value) }),
            header: MemoHeader::new(revision_now, revisions),
        }
    }

    pub(super) fn value<'db>(&'db self) -> Option<&'db C::Output<'db>> {
        self.value
            .as_ref()
            .map(crate::salsa_value::rebind::<C::OutputValue>)
    }

    /// Returns `true` if this memo should be serialized.
    pub(super) fn should_serialize(&self) -> bool {
        // TODO: Serialization is a good opportunity to prune old query results based on
        // the `verified_at` revision.
        self.value.is_some() && !self.header.may_be_provisional()
    }

    pub(super) fn tracing_debug(&self) -> impl std::fmt::Debug + use<'_, C> {
        self.header.tracing_debug(self.value.is_some())
    }
}

impl<C: Configuration> crate::table::memo::Memo for Memo<C> {
    fn remove_outputs(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        self.header.remove_outputs(zalsa, executor);
    }

    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self) -> crate::database::MemoInfo {
        let size_of = std::mem::size_of::<Memo<C>>() + self.header.revisions.allocation_size();
        let heap_size = self.value().map_or(Some(0), C::heap_size);

        crate::database::MemoInfo {
            debug_name: C::DEBUG_NAME,
            output: crate::database::SlotInfo {
                size_of_metadata: size_of - std::mem::size_of::<C::Output<'static>>(),
                debug_name: std::any::type_name::<C::Output<'static>>(),
                size_of_fields: std::mem::size_of::<C::Output<'static>>(),
                heap_size_of_fields: heap_size,
                memos: Vec::new(),
            },
        }
    }
}

#[cfg(feature = "persistence")]
mod persistence {
    use crate::function::Configuration;
    use crate::function::memo::{Memo, MemoHeader};
    use crate::revision::AtomicRevision;
    use crate::zalsa_local::QueryRevisions;
    use crate::zalsa_local::persistence::{MappedQueryRevisions, PersistentQueryOrigin};

    use serde::Deserialize;
    use serde::ser::SerializeStruct;

    /// A reference to the fields of a [`Memo`], with its [`QueryRevisions`] transformed.
    pub(crate) struct MappedMemo<'memo, C: Configuration> {
        pub(crate) value: Option<&'memo C::Output<'memo>>,
        pub(crate) verified_at: AtomicRevision,
        pub(crate) revisions: MappedQueryRevisions<'memo>,
    }

    impl<C: Configuration> Memo<C> {
        pub(crate) fn with_origin(
            &self,
            serialized_origin: PersistentQueryOrigin,
        ) -> MappedMemo<'_, C> {
            let value = self.value();
            let Memo { ref header, .. } = *self;
            let MemoHeader {
                ref verified_at,
                ref revisions,
            } = *header;

            MappedMemo {
                value,
                verified_at: AtomicRevision::from(verified_at.load()),
                revisions: revisions.with_origin(serialized_origin),
            }
        }
    }

    impl<C> serde::Serialize for MappedMemo<'_, C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            struct SerializeValue<'me, 'db, C: Configuration>(&'me C::Output<'db>);

            impl<C> serde::Serialize for SerializeValue<'_, '_, C>
            where
                C: Configuration,
            {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: serde::Serializer,
                {
                    C::serialize(self.0, serializer)
                }
            }

            let MappedMemo {
                value,
                verified_at,
                revisions,
            } = self;

            let value = value.expect(
                "attempted to serialize memo where `Memo::should_serialize` returned `false`",
            );

            let mut s = serializer.serialize_struct("Memo", 3)?;
            s.serialize_field("value", &SerializeValue::<C>(value))?;
            s.serialize_field("verified_at", &verified_at)?;
            s.serialize_field("revisions", &revisions)?;
            s.end()
        }
    }

    impl<'de, C> serde::Deserialize<'de> for Memo<C>
    where
        C: Configuration,
    {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            #[derive(Deserialize)]
            #[serde(rename = "Memo")]
            pub struct DeserializeMemo<C: Configuration> {
                #[serde(bound = "C: Configuration")]
                value: DeserializeValue<C>,
                verified_at: AtomicRevision,
                revisions: QueryRevisions,
            }

            struct DeserializeValue<C: Configuration>(C::Output<'static>);

            impl<'de, C> serde::Deserialize<'de> for DeserializeValue<C>
            where
                C: Configuration,
            {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: serde::Deserializer<'de>,
                {
                    C::deserialize(deserializer)
                        .map(DeserializeValue)
                        .map_err(serde::de::Error::custom)
                }
            }

            let memo = DeserializeMemo::<C>::deserialize(deserializer)?;

            Ok(Memo::new(
                Some(memo.value.0),
                memo.verified_at.load(),
                memo.revisions,
            ))
        }
    }
}

#[derive(Debug)]
pub(super) enum TryClaimHeadsResult {
    /// Claiming the cycle head results in a cycle.
    Cycle {
        head_iteration: IterationStamp,
        memo_iteration: IterationStamp,
        verified_at: Revision,
    },

    /// The cycle head is not finalized, but it can be claimed.
    Available,

    /// The cycle head is currently executed on another thread.
    Running,
}

/// Iterator to try claiming the transitive cycle heads of a memo.
pub(super) struct TryClaimCycleHeadsIter<'a> {
    zalsa: &'a Zalsa,

    cycle_heads: CycleHeadsIterator<'a>,
}

impl<'a> TryClaimCycleHeadsIter<'a> {
    pub(super) fn new(zalsa: &'a Zalsa, cycle_heads: &'a CycleHeads) -> Self {
        Self {
            zalsa,

            cycle_heads: cycle_heads.iter(),
        }
    }
}

impl Iterator for TryClaimCycleHeadsIter<'_> {
    type Item = TryClaimHeadsResult;

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.cycle_heads.next()?;
        let head_database_key = head.database_key_index;
        let head_key_index = head_database_key.key_index();
        let ingredient = self
            .zalsa
            .lookup_ingredient(head_database_key.ingredient_index());

        match ingredient.wait_for(self.zalsa, head_key_index) {
            WaitForResult::Cycle { .. } => {
                // We hit a cycle blocking on the cycle head; this means this query actively
                // participates in the cycle and some other query is blocked on this thread.
                crate::tracing::trace!("Waiting for {head_database_key:?} results in a cycle");

                let provisional_status = ingredient
                    .provisional_status(self.zalsa, head_key_index)
                    .expect("cycle head memo to exist");
                let (current_iteration, verified_at) = match provisional_status {
                    ProvisionalStatus::Provisional {
                        iteration,
                        verified_at,
                        cycle_heads: _,
                    } => (iteration, verified_at),
                    ProvisionalStatus::Final {
                        iteration,
                        verified_at,
                    } => (iteration, verified_at),
                };

                Some(TryClaimHeadsResult::Cycle {
                    memo_iteration: current_iteration,
                    head_iteration: head.iteration.load(),
                    verified_at,
                })
            }
            WaitForResult::Running(running) => {
                crate::tracing::trace!("Ingredient {head_database_key:?} is running: {running:?}");

                Some(TryClaimHeadsResult::Running)
            }
            WaitForResult::Available => Some(TryClaimHeadsResult::Available),
        }
    }
}

#[cfg(all(not(feature = "shuttle"), target_pointer_width = "64"))]
mod _memory_usage {
    use crate::cycle::CycleRecoveryStrategy;
    use crate::ingredient::Location;
    use crate::plumbing::{self, IngredientIndices, MemoIngredientSingletonIndex, SalsaStructInDb};
    use crate::table::memo::MemoTableWithTypes;
    use crate::zalsa::Zalsa;
    use crate::{Database, Id, Revision};

    use std::any::TypeId;
    use std::num::NonZeroUsize;

    // Memo's are stored a lot, make sure their size doesn't randomly increase.
    const _: [(); std::mem::size_of::<super::MemoHeader>()] =
        [(); std::mem::size_of::<[usize; 4]>()];
    const _: [(); std::mem::size_of::<super::Memo<DummyConfiguration>>()] =
        [(); std::mem::size_of::<[usize; 5]>()];

    struct DummyStruct;

    impl SalsaStructInDb for DummyStruct {
        type MemoIngredientMap = MemoIngredientSingletonIndex;
        const LEAF_TYPE_IDS: &'static [typeid::ConstTypeId] = &[];

        fn lookup_ingredient_index(_: &Zalsa) -> IngredientIndices {
            unimplemented!()
        }

        fn cast(_: Id, _: TypeId) -> Option<Self> {
            unimplemented!()
        }

        unsafe fn memo_table(_: &Zalsa, _: Id, _: Revision) -> MemoTableWithTypes<'_> {
            unimplemented!()
        }

        fn entries(_: &Zalsa) -> impl Iterator<Item = crate::DatabaseKeyIndex> + '_ {
            std::iter::empty()
        }
    }

    struct DummyConfiguration;

    impl super::Configuration for DummyConfiguration {
        const DEBUG_NAME: &'static str = "";
        const LOCATION: Location = Location { file: "", line: 0 };
        const PERSIST: bool = false;
        const CYCLE_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::Panic;

        type DbView = dyn Database;
        type SalsaStruct<'db> = DummyStruct;
        type Input<'db> = ();
        type Output<'db> = NonZeroUsize;
        type OutputValue = NonZeroUsize;
        type Eviction = crate::function::eviction::NoopEviction;

        fn values_equal<'db>(_: &Self::Output<'db>, _: &Self::Output<'db>) -> bool {
            unimplemented!()
        }

        fn id_to_input(_: &Zalsa, _: Id) -> Self::Input<'_> {
            unimplemented!()
        }

        fn execute<'db>(_: &'db Self::DbView, _: Self::Input<'db>) -> Self::Output<'db> {
            unimplemented!()
        }

        fn cycle_initial<'db>(
            _: &'db Self::DbView,
            _: Id,
            _: Self::Input<'db>,
        ) -> Self::Output<'db> {
            unimplemented!()
        }

        fn recover_from_cycle<'db>(
            _: &'db Self::DbView,
            _: &crate::Cycle,
            _: &Self::Output<'db>,
            value: Self::Output<'db>,
            _: Self::Input<'db>,
        ) -> Self::Output<'db> {
            value
        }

        fn serialize<S>(_: &Self::Output<'_>, _: S) -> Result<S::Ok, S::Error>
        where
            S: plumbing::serde::Serializer,
        {
            unimplemented!()
        }

        fn deserialize<'de, D>(_: D) -> Result<Self::Output<'static>, D::Error>
        where
            D: plumbing::serde::Deserializer<'de>,
        {
            unimplemented!()
        }
    }
}
