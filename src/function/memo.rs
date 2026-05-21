use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::mem::transmute;
use std::ptr::NonNull;

use crate::function::{Configuration, IngredientImpl};
use crate::key::DatabaseKeyIndex;
use crate::revision::AtomicRevision;
use crate::sync::atomic::Ordering;
use crate::table::memo::MemoTableWithTypesMut;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryOriginRef, QueryRevisions};
use crate::{Event, EventKind, Id, Revision};

impl<C: Configuration> IngredientImpl<C> {
    /// Inserts the memo for the given key; (atomically) overwrites and returns any previously existing memo
    pub(super) fn insert_memo_into_table_for<'db>(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: NonNull<Memo<'db, C>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<Memo<'db, C>>> {
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        let static_memo =
            unsafe { transmute::<NonNull<Memo<'db, C>>, NonNull<Memo<'static, C>>>(memo) };
        let old_static_memo = zalsa
            .memo_table_for::<C::SalsaStruct<'_>>(id)
            .insert(memo_ingredient_index, static_memo)?;
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        Some(unsafe {
            transmute::<NonNull<Memo<'static, C>>, NonNull<Memo<'db, C>>>(old_static_memo)
        })
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        let static_memo = zalsa
            .memo_table_for::<C::SalsaStruct<'_>>(id)
            .get(memo_ingredient_index)?;
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        Some(unsafe { transmute::<&Memo<'static, C>, &'db Memo<'db, C>>(static_memo.as_ref()) })
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: MemoTableWithTypesMut<'_>,
        memo_ingredient_index: MemoIngredientIndex,
    ) {
        let map = |memo: &mut Memo<'static, C>| {
            match memo.revisions.origin.as_ref() {
                QueryOriginRef::Assigned(_) | QueryOriginRef::DerivedUntracked(_) => {
                    // Careful: Cannot evict memos whose values were
                    // assigned as output of another query
                    // or those with untracked inputs
                    // as their values cannot be reconstructed.
                }
                QueryOriginRef::Derived(_) => {
                    // Set the memo value to `None`.
                    memo.value = None;
                }
            }
        };

        table.map_memo(memo_ingredient_index, map)
    }
}

#[derive(Debug)]
pub struct Memo<'db, C: Configuration> {
    /// The result of the query, if we decide to memoize it.
    pub(super) value: Option<C::Output<'db>>,

    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicRevision,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

impl<'db, C: Configuration> Memo<'db, C> {
    pub(super) fn new(
        value: Option<C::Output<'db>>,
        revision_now: Revision,
        revisions: QueryRevisions,
    ) -> Self {
        Memo {
            value,
            verified_at: AtomicRevision::from(revision_now),
            revisions,
        }
    }

    pub(super) fn with_active_cycle(
        mut self,
        zalsa: &crate::zalsa::Zalsa,
        database_key_index: DatabaseKeyIndex,
        active_cycle: crate::active_cycle::ActiveCycleKey,
    ) -> Self {
        self.revisions.set_active_cycle(active_cycle);
        zalsa
            .active_cycles()
            .add_participant(active_cycle, database_key_index);
        self
    }

    /// Returns `true` if this memo should be serialized.
    pub(super) fn should_serialize(&self) -> bool {
        // TODO: Serialization is a good opportunity to prune old query results based on
        // the `verified_at` revision.
        self.value.is_some() && !self.may_be_provisional()
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

    /// Returns `true` if this memo was part of a cycle in it's last iteration.
    #[inline(always)]
    pub(super) fn was_cycle_participant(&self) -> bool {
        self.revisions.active_cycle().is_some()
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
        for output in self.revisions.origin.as_ref().outputs() {
            output.mark_validated_output(zalsa, database_key_index);
        }
    }

    pub(super) fn tracing_debug(&self) -> impl std::fmt::Debug + use<'_, 'db, C> {
        struct TracingDebug<'memo, 'db, C: Configuration> {
            memo: &'memo Memo<'db, C>,
        }

        impl<C: Configuration> std::fmt::Debug for TracingDebug<'_, '_, C> {
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

impl<C: Configuration> crate::table::memo::Memo for Memo<'static, C>
where
    C::Output<'static>: Send + Sync + Any,
{
    fn remove_outputs(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex) {
        for stale_output in self.revisions.origin.as_ref().outputs() {
            stale_output.remove_stale_output(zalsa, executor);
        }

        for (identity, id) in self.revisions.tracked_struct_ids() {
            let key = DatabaseKeyIndex::new(identity.ingredient_index(), *id);
            key.remove_stale_output(zalsa, executor);
        }
    }

    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self) -> crate::database::MemoInfo {
        let size_of = std::mem::size_of::<Memo<C>>() + self.revisions.allocation_size();
        let heap_size = if let Some(value) = self.value.as_ref() {
            C::heap_size(value)
        } else {
            Some(0)
        };

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
    use crate::function::memo::Memo;
    use crate::revision::AtomicRevision;
    use crate::zalsa_local::persistence::MappedQueryRevisions;
    use crate::zalsa_local::{QueryOrigin, QueryRevisions};

    use serde::Deserialize;
    use serde::ser::SerializeStruct;

    /// A reference to the fields of a [`Memo`], with its [`QueryRevisions`] transformed.
    pub(crate) struct MappedMemo<'memo, 'db, C: Configuration> {
        pub(crate) value: Option<&'memo C::Output<'db>>,
        pub(crate) verified_at: AtomicRevision,
        pub(crate) revisions: MappedQueryRevisions<'memo>,
    }

    impl<'db, C: Configuration> Memo<'db, C> {
        pub(crate) fn with_origin(&self, origin: QueryOrigin) -> MappedMemo<'_, 'db, C> {
            let Memo {
                ref verified_at,
                ref value,
                ref revisions,
            } = *self;

            MappedMemo {
                value: value.as_ref(),
                verified_at: AtomicRevision::from(verified_at.load()),
                revisions: revisions.with_origin(origin),
            }
        }
    }

    impl<C> serde::Serialize for MappedMemo<'_, '_, C>
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

    impl<'de, C> serde::Deserialize<'de> for Memo<'static, C>
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

            Ok(Memo {
                value: Some(memo.value.0),
                verified_at: memo.verified_at,
                revisions: memo.revisions,
            })
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
    const _: [(); std::mem::size_of::<super::Memo<DummyConfiguration>>()] =
        [(); std::mem::size_of::<[usize; 6]>()];

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
