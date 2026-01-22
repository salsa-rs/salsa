//! Cycle handling
//!
//! Salsa's default cycle handling is quite simple: if we encounter a cycle (that is, if we attempt
//! to execute a query that is already on the active query stack), we panic.
//!
//! By setting `cycle_fn` and `cycle_initial` arguments to `salsa::tracked`, queries can opt-in to
//! fixed-point iteration instead.
//!
//! We call the query which triggers the cycle (that is, the query that is already on the stack
//! when it is called again) the "cycle head". The cycle head is responsible for managing iteration
//! of the cycle. When a cycle is encountered, if the cycle head has `cycle_fn` and `cycle_initial`
//! set, it will call the `cycle_initial` function to generate an "empty" or "initial" value for
//! fixed-point iteration, which will be returned to its caller. Then each query in the cycle will
//! compute a value normally, but every computed value will track the head(s) of the cycles it is
//! part of. Every query's "cycle heads" are the union of all the cycle heads of all the queries it
//! depends on. A memoized query result with cycle heads is called a "provisional value".
//!
//! For example, if `qa` calls `qb`, and `qb` calls `qc`, and `qc` calls `qa`, then `qa` will call
//! its `cycle_initial` function to get an initial value, and return that as its result to `qc`,
//! marked with `qa` as cycle head. `qc` will compute its own provisional result based on that, and
//! return to `qb` a result also marked with `qa` as cycle head. `qb` will similarly compute and
//! return a provisional value back to `qa`.
//!
//! When a query observes that it has just computed a result which contains itself as a cycle head,
//! it recognizes that it is responsible for resolving this cycle and calls its `cycle_fn` to
//! decide what value to use. The `cycle_fn` function is passed the provisional value just computed
//! for that query and the count of iterations so far, and returns the value to use for this
//! iteration. This can be the computed value itself, or a different value (e.g., a fallback value).
//!
//! If the cycle head ever observes that the value returned by `cycle_fn` is the same as the
//! provisional value from the previous iteration, this cycle has converged. The cycle head will
//! mark that value as final (by removing itself as cycle head) and return it.
//!
//! Other queries in the cycle will still have provisional values recorded, but those values should
//! now also be considered final! We don't eagerly walk the entire cycle to mark them final.
//! Instead, we wait until the next time that provisional value is read, and then we check if all
//! of its cycle heads have a final result, in which case it, too, can be marked final. (This is
//! implemented in `shallow_verify_memo` and `validate_provisional`.)
//!
//! In nested cycle cases, the inner cycles are iterated as part of the outer cycle iteration. This helps
//! to significantly reduce the number of iterations needed to reach a fixpoint. For nested cycles,
//! the inner cycles head will transfer their lock ownership to the outer cycle. This ensures
//! that, over time, the outer cycle will hold all necessary locks to complete the fixpoint iteration.
//! Without this, different threads would compete for the locks of inner cycle heads, leading to potential
//! hangs (but not deadlocks).

use std::iter::FusedIterator;
use thin_vec::{thin_vec, ThinVec};

use crate::key::DatabaseKeyIndex;
use crate::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use crate::sync::OnceLock;
use crate::{Id, Revision};

/// The maximum number of times we'll fixpoint-iterate before panicking.
///
/// Should only be relevant in case of a badly configured cycle recovery.
pub const MAX_ITERATIONS: IterationCount = IterationCount(200);

/// Cycle recovery strategy: Is this query capable of recovering from
/// a cycle that results from executing the function? If so, how?
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CycleRecoveryStrategy {
    /// Cannot recover from cycles: panic.
    ///
    /// This is the default.
    Panic,

    /// Recovers from cycles by fixpoint iterating and/or falling
    /// back to a sentinel value.
    ///
    /// This choice is computed by the query's `cycle_recovery`
    /// function and initial value.
    Fixpoint,

    /// Recovers from cycles by inserting a fallback value for all
    /// queries that have a fallback, and ignoring any other query
    /// in the cycle (as if they were not computed).
    FallbackImmediate,
}

/// A "cycle head" is the query at which we encounter a cycle; that is, if A -> B -> C -> A, then A
/// would be the cycle head. It returns an "initial value" when the cycle is encountered (if
/// fixpoint iteration is enabled for that query), and then is responsible for re-iterating the
/// cycle until it converges.
#[derive(Debug)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
pub struct CycleHead {
    pub(crate) database_key_index: DatabaseKeyIndex,
    pub(crate) iteration_count: AtomicIterationCount,

    /// Marks a cycle head as removed within its `CycleHeads` container.
    ///
    /// Cycle heads are marked as removed when the memo from the last iteration (a provisional memo)
    /// is used as the initial value for the next iteration. It's necessary to remove all but its own
    /// head from the `CycleHeads` container, because the query might now depend on fewer cycles
    /// (in case of conditional dependencies). However, we can't actually remove the cycle head
    /// within `fetch_cold_cycle` because we only have a readonly memo. That's what `removed` is used for.
    #[cfg_attr(feature = "persistence", serde(skip))]
    removed: AtomicBool,
}

impl CycleHead {
    pub const fn new(
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) -> Self {
        Self {
            database_key_index,
            iteration_count: AtomicIterationCount(AtomicU8::new(iteration_count.0)),
            removed: AtomicBool::new(false),
        }
    }
}

impl Clone for CycleHead {
    fn clone(&self) -> Self {
        Self {
            database_key_index: self.database_key_index,
            iteration_count: self.iteration_count.load().into(),
            removed: self.removed.load(Ordering::Relaxed).into(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "persistence", serde(transparent))]
pub struct IterationCount(u8);

impl IterationCount {
    pub(crate) const fn initial() -> Self {
        Self(0)
    }

    pub(crate) const fn is_initial(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn increment(self) -> Option<Self> {
        let next = Self(self.0 + 1);
        if next.0 <= MAX_ITERATIONS.0 {
            Some(next)
        } else {
            None
        }
    }

    pub(crate) const fn as_u32(self) -> u32 {
        self.0 as u32
    }
}

impl std::fmt::Display for IterationCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug)]
pub(crate) struct AtomicIterationCount(AtomicU8);

impl AtomicIterationCount {
    pub(crate) fn load(&self) -> IterationCount {
        IterationCount(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn load_mut(&mut self) -> IterationCount {
        IterationCount(*self.0.get_mut())
    }

    pub(crate) fn store(&self, value: IterationCount) {
        self.0.store(value.0, Ordering::Release);
    }

    pub(crate) fn store_mut(&mut self, value: IterationCount) {
        *self.0.get_mut() = value.0;
    }
}

impl std::fmt::Display for AtomicIterationCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.load().fmt(f)
    }
}

impl From<IterationCount> for AtomicIterationCount {
    fn from(iteration_count: IterationCount) -> Self {
        AtomicIterationCount(iteration_count.0.into())
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for AtomicIterationCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.load().serialize(serializer)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for AtomicIterationCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        IterationCount::deserialize(deserializer).map(Into::into)
    }
}

/// Any provisional value generated by any query in a cycle will track the cycle head(s) (can be
/// plural in case of nested cycles) representing the cycles it is part of, and the current
/// iteration count for each cycle head. This struct tracks these cycle heads.
#[derive(Clone, Debug, Default)]
pub struct CycleHeads(ThinVec<CycleHead>);

impl CycleHeads {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn initial(
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) -> Self {
        Self(thin_vec![CycleHead {
            database_key_index,
            iteration_count: iteration_count.into(),
            removed: false.into()
        }])
    }

    pub(crate) fn iter(&self) -> CycleHeadsIterator<'_> {
        CycleHeadsIterator {
            inner: self.0.iter(),
        }
    }

    pub(crate) fn ids(&self) -> CycleHeadIdsIterator<'_> {
        CycleHeadIdsIterator { inner: self.iter() }
    }

    /// Iterates over all cycle heads that aren't equal to `own`.
    pub(crate) fn iter_not_eq(
        &self,
        own: DatabaseKeyIndex,
    ) -> impl DoubleEndedIterator<Item = &CycleHead> {
        self.iter()
            .filter(move |head| head.database_key_index != own)
    }

    pub(crate) fn contains(&self, value: &DatabaseKeyIndex) -> bool {
        self.into_iter()
            .any(|head| head.database_key_index == *value)
    }

    /// Removes all cycle heads except `except` by marking them as removed.
    ///
    /// Note that the heads aren't actually removed. They're only marked as removed and will be
    /// skipped when iterating. This is because we might not have a mutable reference.
    pub(crate) fn remove_all_except(&self, except: DatabaseKeyIndex) {
        for head in self.0.iter() {
            if head.database_key_index == except {
                continue;
            }

            head.removed.store(true, Ordering::Release);
        }
    }

    /// Updates the iteration count for the head `cycle_head_index` to `new_iteration_count`.
    ///
    /// Unlike [`update_iteration_count`], this method takes a `&mut self` reference. It should
    /// be preferred if possible, as it avoids atomic operations.
    pub(crate) fn update_iteration_count_mut(
        &mut self,
        cycle_head_index: DatabaseKeyIndex,
        new_iteration_count: IterationCount,
    ) {
        if let Some(cycle_head) = self
            .0
            .iter_mut()
            .find(|cycle_head| cycle_head.database_key_index == cycle_head_index)
        {
            cycle_head.iteration_count.store_mut(new_iteration_count);
        }
    }

    /// Updates the iteration count for the head `cycle_head_index` to `new_iteration_count`.
    ///
    /// Unlike [`update_iteration_count_mut`], this method takes a `&self` reference.
    pub(crate) fn update_iteration_count(
        &self,
        cycle_head_index: DatabaseKeyIndex,
        new_iteration_count: IterationCount,
    ) {
        if let Some(cycle_head) = self
            .0
            .iter()
            .find(|cycle_head| cycle_head.database_key_index == cycle_head_index)
        {
            cycle_head.iteration_count.store(new_iteration_count);
        }
    }

    #[inline]
    pub(crate) fn extend(&mut self, other: &Self) {
        self.0.reserve(other.0.len());

        for head in other {
            debug_assert!(!head.removed.load(Ordering::Relaxed));
            self.insert(head.database_key_index, head.iteration_count.load());
        }
    }

    pub(crate) fn insert(
        &mut self,
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) -> bool {
        if let Some(existing) = self
            .0
            .iter_mut()
            .find(|candidate| candidate.database_key_index == database_key_index)
        {
            let removed = existing.removed.get_mut();

            if *removed {
                *removed = false;
                existing.iteration_count.store_mut(iteration_count);

                true
            } else {
                let existing_count = existing.iteration_count.load_mut();

                assert_eq!(
                    existing_count, iteration_count,
                    "Can't merge cycle heads {:?} with different iteration counts ({existing_count:?}, {iteration_count:?})",
                    existing.database_key_index
                );

                false
            }
        } else {
            self.0
                .push(CycleHead::new(database_key_index, iteration_count));
            true
        }
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn allocation_size(&self) -> usize {
        std::mem::size_of_val(self.0.as_slice())
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for CycleHeads {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;

        let mut seq = serializer.serialize_seq(None)?;
        for e in self {
            if e.removed.load(Ordering::Relaxed) {
                continue;
            }

            seq.serialize_element(e)?;
        }
        seq.end()
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for CycleHeads {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let vec: ThinVec<CycleHead> = serde::Deserialize::deserialize(deserializer)?;
        Ok(CycleHeads(vec))
    }
}

impl IntoIterator for CycleHeads {
    type Item = CycleHead;
    type IntoIter = <ThinVec<Self::Item> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Clone)]
pub struct CycleHeadsIterator<'a> {
    inner: std::slice::Iter<'a, CycleHead>,
}

impl<'a> Iterator for CycleHeadsIterator<'a> {
    type Item = &'a CycleHead;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next = self.inner.next()?;

            if next.removed.load(Ordering::Relaxed) {
                continue;
            }

            return Some(next);
        }
    }
}

impl FusedIterator for CycleHeadsIterator<'_> {}
impl DoubleEndedIterator for CycleHeadsIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            let next = self.inner.next_back()?;

            if next.removed.load(Ordering::Relaxed) {
                continue;
            }

            return Some(next);
        }
    }
}

impl<'a> std::iter::IntoIterator for &'a CycleHeads {
    type Item = &'a CycleHead;
    type IntoIter = CycleHeadsIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl From<CycleHead> for CycleHeads {
    fn from(value: CycleHead) -> Self {
        Self(thin_vec![value])
    }
}

#[inline]
pub(crate) fn empty_cycle_heads() -> &'static CycleHeads {
    static EMPTY_CYCLE_HEADS: OnceLock<CycleHeads> = OnceLock::new();
    EMPTY_CYCLE_HEADS.get_or_init(|| CycleHeads(ThinVec::new()))
}

#[derive(Clone)]
pub struct CycleHeadIdsIterator<'a> {
    inner: CycleHeadsIterator<'a>,
}

impl Iterator for CycleHeadIdsIterator<'_> {
    type Item = crate::Id;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|head| head.database_key_index.key_index())
    }
}

/// The context that the cycle recovery function receives when a query cycle occurs.
pub struct Cycle<'a> {
    pub(crate) head_ids: CycleHeadIdsIterator<'a>,
    pub(crate) id: Id,
    pub(crate) iteration: u32,
}

impl Cycle<'_> {
    /// An iterator that outputs the [`Id`]s of the current cycle heads.
    /// This always contains the [`Id`] of the current query but it can contain additional cycle head [`Id`]s
    /// if this query is nested in an outer cycle or if it has nested cycles.
    pub fn head_ids(&self) -> CycleHeadIdsIterator<'_> {
        self.head_ids.clone()
    }

    /// The [`Id`] of the query that the current cycle recovery function is processing.
    pub fn id(&self) -> Id {
        self.id
    }

    /// The counter of the current fixed point iteration.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }
}

#[derive(Debug)]
pub enum ProvisionalStatus<'db> {
    Provisional {
        iteration: IterationCount,
        verified_at: Revision,
        cycle_heads: &'db CycleHeads,
    },
    Final {
        iteration: IterationCount,
        verified_at: Revision,
    },
}

impl<'db> ProvisionalStatus<'db> {
    pub(crate) fn cycle_heads(&self) -> &'db CycleHeads {
        match self {
            ProvisionalStatus::Provisional { cycle_heads, .. } => cycle_heads,
            _ => empty_cycle_heads(),
        }
    }

    pub(crate) const fn is_provisional(&self) -> bool {
        matches!(self, ProvisionalStatus::Provisional { .. })
    }
}
