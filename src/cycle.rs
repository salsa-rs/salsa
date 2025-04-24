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
//! decide how to do so. The `cycle_fn` function is passed the provisional value just computed for
//! that query and the count of iterations so far, and must return either
//! `CycleRecoveryAction::Iterate` (which signals that the cycle head should re-iterate the cycle),
//! or `CycleRecoveryAction::Fallback` (which signals that the cycle head should replace its
//! computed value with the given fallback value).
//!
//! If the cycle head ever observes that the provisional value it just recomputed is the same as
//! the provisional value from the previous iteration, the cycle has converged. The cycle head will
//! mark that value as final (by removing itself as cycle head) and return it.
//!
//! Other queries in the cycle will still have provisional values recorded, but those values should
//! now also be considered final! We don't eagerly walk the entire cycle to mark them final.
//! Instead, we wait until the next time that provisional value is read, and then we check if all
//! of its cycle heads have a final result, in which case it, too, can be marked final. (This is
//! implemented in `shallow_verify_memo` and `validate_provisional`.)
//!
//! If the `cycle_fn` returns a fallback value, the cycle head will replace its provisional value
//! with that fallback, and then iterate the cycle one more time. A fallback value is expected to
//! result in a stable, converged cycle. If it does not (that is, if the result of another
//! iteration of the cycle is not the same as the fallback value), we'll panic.
//!
//! In nested cycle cases, the inner cycle head will iterate until its own cycle is resolved, but
//! the "final" value it then returns will still be provisional on the outer cycle head. The outer
//! cycle head may then iterate, which may result in a new set of iterations on the inner cycle,
//! for each iteration of the outer cycle.

use smallvec::{smallvec, SmallVec};

use crate::hash::FxDashMap;
use crate::key::DatabaseKeyIndex;

/// The maximum number of times we'll fixpoint-iterate before panicking.
///
/// Should only be relevant in case of a badly configured cycle recovery.
pub const MAX_ITERATIONS: u32 = 200;

/// Return value from a cycle recovery function.
#[derive(Debug)]
pub enum CycleRecoveryAction<T> {
    /// Iterate the cycle again to look for a fixpoint.
    Iterate,

    /// Cut off iteration and use the given result value for this query.
    Fallback(T),
}

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
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CycleHead {
    pub(crate) database_key_index: DatabaseKeyIndex,
    pub(crate) iteration_count: u32,
}

/// Any provisional value generated by any query in a cycle will track the cycle head(s) (can be
/// plural in case of nested cycles) representing the cycles it is part of, and the current
/// iteration count for each cycle head. This struct tracks these cycle heads.
#[derive(Clone, Debug, Default)]
pub struct CycleHeads(SmallVec<[CycleHead; 1]>);

impl CycleHeads {
    pub(crate) const EMPTY: &'static Self = &CycleHeads(SmallVec::new_const());

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn initial(database_key_index: DatabaseKeyIndex) -> Self {
        Self(smallvec![CycleHead {
            database_key_index,
            iteration_count: 0,
        }])
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, CycleHead> {
        self.0.iter()
    }

    pub(crate) fn contains(&self, value: DatabaseKeyIndex) -> bool {
        self.into_iter()
            .any(|head| head.database_key_index == value)
    }

    pub(crate) fn remove(&mut self, value: &DatabaseKeyIndex) -> bool {
        let found = self
            .0
            .iter()
            .position(|&head| head.database_key_index == *value);
        let Some(found) = found else { return false };
        self.0.swap_remove(found);
        true
    }

    pub(crate) fn update_iteration_count(
        &mut self,
        cycle_head_index: DatabaseKeyIndex,
        new_iteration_count: u32,
    ) {
        if let Some(cycle_head) = self
            .0
            .iter_mut()
            .find(|cycle_head| cycle_head.database_key_index == cycle_head_index)
        {
            cycle_head.iteration_count = new_iteration_count;
        }
    }

    #[inline]
    pub(crate) fn extend(&mut self, other: &Self) {
        self.0.reserve(other.0.len());

        for head in other {
            if let Some(existing) = self
                .0
                .iter()
                .find(|candidate| candidate.database_key_index == head.database_key_index)
            {
                assert!(existing.iteration_count == head.iteration_count);
            } else {
                self.0.push(*head);
            }
        }
    }
}

impl IntoIterator for CycleHeads {
    type Item = CycleHead;
    type IntoIter = smallvec::IntoIter<[CycleHead; 1]>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> std::iter::IntoIterator for &'a CycleHeads {
    type Item = &'a CycleHead;
    type IntoIter = std::slice::Iter<'a, CycleHead>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl From<CycleHead> for CycleHeads {
    fn from(value: CycleHead) -> Self {
        Self(smallvec![value])
    }
}

/// A query is only stored here if it is a part of a cycle, and removed as we verify it.
/// This is stored as a side map and not in `Memo` to save space (most queries don't have
/// cycles).
///
/// Subtle: in addition to the `CycleHeads`, this store the address of the memo (this is the
/// `usize`). This is needed because sometimes, we retrieve a memo, then verify it. During
/// this verification we enter a cycle, and a new, non-empty `CycleHeads` is inserted.
/// Then, we complete the verification of the (old) memo, and find it up-to-date. Therefore,
/// we remove it from this map. But the map now contains the `CycleHeads` of the **new**
/// memo, and now we may verify it incorrectly! To prevent this, we only remove the `CycleHeads`
/// if the memo matches.
///
/// This result was computed based on provisional values from
/// these cycle heads. The "cycle head" is the query responsible
/// for managing a fixpoint iteration. In a cycle like
/// `--> A --> B --> C --> A`, the cycle head is query `A`: it is
/// the query whose value is requested while it is executing,
/// which must provide the initial provisional value and decide,
/// after each iteration, whether the cycle has converged or must
/// iterate again.
#[derive(Default)]
pub(crate) struct CycleHeadsMap(FxDashMap<DatabaseKeyIndex, (CycleHeads, usize)>);

impl CycleHeadsMap {
    /// Only call this after checking `may_be_provisional()`.
    #[cold]
    pub(crate) fn cycle_heads_contain(&self, key: DatabaseKeyIndex) -> bool {
        self.0
            .get(&key)
            .is_some_and(|cycle_heads| cycle_heads.0.contains(key))
    }

    /// Only call this after checking `may_be_provisional()`.
    #[cold]
    pub(crate) fn insert(&self, key: DatabaseKeyIndex, cycle_heads: CycleHeads, memo: *const ()) {
        self.0.insert(key, (cycle_heads, ptr_addr(memo)));
    }

    /// Only call this after checking `may_be_provisional()`.
    #[cold]
    pub(crate) fn remove(&self, key: DatabaseKeyIndex, memo: *const ()) {
        self.0
            .remove_if(&key, |_, &(_, memo_addr)| memo_addr == ptr_addr(memo));
    }

    /// Remove without checking the memo is the same.
    #[cold]
    pub(crate) fn always_remove(&self, key: DatabaseKeyIndex) {
        self.0.remove(&key);
    }

    /// Only call this after checking `may_be_provisional()`.
    #[cold]
    pub(crate) fn get_cloned(&self, key: DatabaseKeyIndex) -> Option<CycleHeads> {
        self.0.get(&key).map(|it| it.0.clone())
    }
}

/// A copy of `<*const T>::addr()`.
// FIXME: Use it when our MSRV is high enough.
#[inline]
#[allow(clippy::transmutes_expressible_as_ptr_casts)] // A cast will expose provenance and cause problems for Miri
fn ptr_addr(ptr: *const ()) -> usize {
    // SAFETY: We just get the address.
    unsafe { std::mem::transmute::<*const (), usize>(ptr) }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CycleHeadKind {
    Provisional,
    NotProvisional,
    FallbackImmediate,
}
