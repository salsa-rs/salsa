use std::panic::AssertUnwindSafe;

use crate::Cycle;

pub(crate) struct CycleParticipant {
    cycle: Cycle,
}

impl CycleParticipant {
    pub(crate) fn new(cycle: Cycle) -> Self {
        Self { cycle }
    }

    pub(crate) fn throw(self) {
        std::panic::resume_unwind(Box::new(self));
    }

    pub(crate) fn recover<T>(execute: impl FnOnce() -> T, recover: impl FnOnce(Cycle) -> T) -> T {
        std::panic::catch_unwind(AssertUnwindSafe(execute)).unwrap_or_else(|err| {
            match err.downcast::<CycleParticipant>() {
                Ok(participant) => recover(participant.cycle),
                Err(v) => std::panic::resume_unwind(v),
            }
        })
    }
}
