use crate::{with_attached_database, Cycle};
use drop_bomb::DebugDropBomb;
use std::fmt;
use std::fmt::Debug;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    kind: Box<ErrorKind>,
}

impl Error {
    pub(crate) fn cancelled(reason: Cancelled) -> Self {
        Error {
            kind: Box::new(ErrorKind::Cancelled(CancelledError {
                reason,
                bomb: DebugDropBomb::new("Cancellation errors must be propagated inside salsa queries. If you see this message outside a salsa query, please open an issue."),
            })),
        }
    }

    pub(crate) fn cycle(cycle: Cycle) -> Self {
        Self {
            kind: Box::new(ErrorKind::Cycle(CycleError {
                cycle,
                bomb: DebugDropBomb::new("Cycle errors must be propagated so that Salsa can resolve the cycle. If you see this message outside a salsa query, please open an issue."),
            })),
        }
    }

    pub(crate) fn into_cycle(self) -> std::result::Result<Cycle, Self> {
        match *self.kind {
            ErrorKind::Cycle(cycle) => Ok(cycle.take_cycle()),
            _ => Err(self),
        }
    }
}

impl From<CycleError> for Error {
    fn from(value: CycleError) -> Self {
        Self {
            kind: Box::new(ErrorKind::Cycle(value)),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.kind {
            ErrorKind::Cycle(cycle) => {
                write!(f, "cycle detected: {:?}", cycle)
            }
            ErrorKind::Cancelled(cancelled) => std::fmt::Display::fmt(cancelled, f),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Cycle(CycleError),
    Cancelled(CancelledError),
}

#[derive(Debug)]
pub(crate) struct CycleError {
    cycle: Cycle,
    bomb: DebugDropBomb,
}

impl CycleError {
    pub(crate) fn take_cycle(mut self) -> Cycle {
        self.bomb.defuse();
        self.cycle
    }
}

#[derive(Debug)]
pub(crate) struct CancelledError {
    reason: Cancelled,
    bomb: DebugDropBomb,
}

impl Drop for CancelledError {
    fn drop(&mut self) {
        if !self.bomb.is_defused() && with_attached_database(|_| {}).is_none() {
            // We are outside a query. It's okay if the user drops the error now
            self.bomb.defuse();
        }
    }
}

/// A panic payload indicating that execution of a salsa query was cancelled.
#[derive(Debug)]
pub(crate) enum Cancelled {
    /// The query was operating on revision R, but there is a pending write to move to revision R+1.
    PendingWrite,

    /// The query was blocked on another thread, and that thread panicked.
    PropagatedPanic,
}

impl std::fmt::Display for CancelledError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = match self.reason {
            Cancelled::PendingWrite => "pending write",
            Cancelled::PropagatedPanic => "propagated panic",
        };
        f.write_str("cancelled because of ")?;
        f.write_str(why)
    }
}
