use crate::{key::DatabaseKeyIndex, Database};
use std::{panic::AssertUnwindSafe, sync::Arc};

/// Captures the participants of a cycle that occurred when executing a query.
///
/// This type is meant to be used to help give meaningful error messages to the
/// user or to help salsa developers figure out why their program is resulting
/// in a computation cycle.
///
/// It is used in a few ways:
///
/// * During [cycle recovery](https://https://salsa-rs.github.io/salsa/cycles/fallback.html),
///   where it is given to the fallback function.
/// * As the panic value when an unexpected cycle (i.e., a cycle where one or more participants
///   lacks cycle recovery information) occurs.
///
/// You can read more about cycle handling in
/// the [salsa book](https://https://salsa-rs.github.io/salsa/cycles.html).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycle {
    participants: CycleParticipants,
}

pub(crate) type CycleParticipants = Arc<Vec<DatabaseKeyIndex>>;

impl Cycle {
    pub(crate) fn new(participants: CycleParticipants) -> Self {
        Self { participants }
    }

    /// True if two `Cycle` values represent the same cycle.
    pub(crate) fn is(&self, cycle: &Cycle) -> bool {
        Arc::ptr_eq(&self.participants, &cycle.participants)
    }

    pub(crate) fn throw(self) -> ! {
        tracing::debug!("throwing cycle {:?}", self);
        std::panic::resume_unwind(Box::new(self))
    }

    pub(crate) fn catch<T>(execute: impl FnOnce() -> T) -> Result<T, Cycle> {
        match std::panic::catch_unwind(AssertUnwindSafe(execute)) {
            Ok(v) => Ok(v),
            Err(err) => match err.downcast::<Cycle>() {
                Ok(cycle) => Err(*cycle),
                Err(other) => std::panic::resume_unwind(other),
            },
        }
    }

    /// Iterate over the [`DatabaseKeyIndex`] for each query participating
    /// in the cycle. The start point of this iteration within the cycle
    /// is arbitrary but deterministic, but the ordering is otherwise determined
    /// by the execution.
    pub fn participant_keys(&self) -> impl Iterator<Item = DatabaseKeyIndex> + '_ {
        self.participants.iter().copied()
    }

    /// Returns a vector with the debug information for
    /// all the participants in the cycle.
    pub fn all_participants(&self, _db: &dyn Database) -> Vec<DatabaseKeyIndex> {
        self.participant_keys().collect()
    }

    /// Returns a vector with the debug information for
    /// those participants in the cycle that lacked recovery
    /// information.
    pub fn unexpected_participants(&self, db: &dyn Database) -> Vec<DatabaseKeyIndex> {
        self.participant_keys()
            .filter(|&d| d.cycle_recovery_strategy(db) == CycleRecoveryStrategy::Panic)
            .collect()
    }
}

impl std::fmt::Debug for Cycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::attach::with_attached_database(|db| {
            f.debug_struct("UnexpectedCycle")
                .field("all_participants", &self.all_participants(db))
                .field("unexpected_participants", &self.unexpected_participants(db))
                .finish()
        })
        .unwrap_or_else(|| {
            f.debug_struct("Cycle")
                .field("participants", &self.participants)
                .finish()
        })
    }
}

/// Cycle recovery strategy: Is this query capable of recovering from
/// a cycle that results from executing the function? If so, how?
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CycleRecoveryStrategy {
    /// Cannot recover from cycles: panic.
    ///
    /// This is the default.
    ///
    /// In the case of a failure due to a cycle, the panic
    /// value will be the `Cycle`.
    Panic,

    /// Recovers from cycles by storing a sentinel value.
    ///
    /// This value is computed by the query's `recovery_fn`
    /// function.
    Fallback,
}
