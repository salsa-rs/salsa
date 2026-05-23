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
//! compute a value normally, but every computed value records the active cycle head(s) it observed
//! while executing. A memoized query result that still depends on an active cycle is called a
//! "provisional value".
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
//! now also be considered final. Salsa tracks the current cycle participants in central active-cycle
//! state, so cycle completion can eagerly mark the current participants final and remove the active
//! cycle state. If a cycle aborts instead, removing the central state makes those provisional memos
//! stale and they re-execute on the next read.
//!
//! In nested cycle cases, the inner cycles are iterated as part of the outer cycle iteration. This helps
//! to significantly reduce the number of iterations needed to reach a fixpoint. For nested cycles,
//! the inner cycles head will transfer their lock ownership to the outer cycle. This ensures
//! that, over time, the outer cycle will hold all necessary locks to complete the fixpoint iteration.
//! Without this, different threads would compete for the locks of inner cycle heads, leading to potential
//! hangs (but not deadlocks).

use std::iter::FusedIterator;

use smallvec::SmallVec;

use crate::Id;
use crate::key::DatabaseKeyIndex;

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
#[derive(Clone, Debug)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
pub struct CycleHead {
    pub(crate) database_key_index: DatabaseKeyIndex,
}

impl CycleHead {
    pub const fn new(database_key_index: DatabaseKeyIndex) -> Self {
        Self { database_key_index }
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

/// Any provisional value generated by any query in a cycle will track the cycle head(s) (can be
/// plural in case of nested cycles) representing the cycles it is part of.
#[derive(Clone, Debug, Default)]
pub struct CycleHeads(SmallVec<[CycleHead; 3]>);

impl CycleHeads {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
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

    #[inline]
    pub(crate) fn extend(&mut self, other: &Self) {
        self.0.reserve(other.0.len());

        for head in other {
            self.insert(head.database_key_index);
        }
    }

    pub(crate) fn insert(&mut self, database_key_index: DatabaseKeyIndex) -> bool {
        if self
            .0
            .iter()
            .any(|candidate| candidate.database_key_index == database_key_index)
        {
            false
        } else {
            self.0.push(CycleHead::new(database_key_index));
            true
        }
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
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
        let vec: Vec<CycleHead> = serde::Deserialize::deserialize(deserializer)?;
        Ok(CycleHeads(vec.into_iter().collect()))
    }
}

impl IntoIterator for CycleHeads {
    type Item = CycleHead;
    type IntoIter = <SmallVec<[Self::Item; 3]> as IntoIterator>::IntoIter;

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
        self.inner.next()
    }
}

impl FusedIterator for CycleHeadsIterator<'_> {}
impl DoubleEndedIterator for CycleHeadsIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
}

impl<'a> std::iter::IntoIterator for &'a CycleHeads {
    type Item = &'a CycleHead;
    type IntoIter = CycleHeadsIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
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
