use crate::DatabaseKeyIndex;

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
}

/// A query cycle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cycle {
    /// The head of the cycle.
    ///
    /// The query whose execution ultimately resulted in calling itself again.
    head: DatabaseKeyIndex,
}
