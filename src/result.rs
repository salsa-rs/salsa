use crate::Cycle;
use drop_bomb::DropBomb;
use std::fmt;
use std::fmt::Debug;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]

pub struct Error {
    kind: ErrorKind,
}

impl Error {
    pub(crate) fn cancelled(reason: Cancelled) -> Self {
        Error {
            kind: ErrorKind::Cancelled(reason),
        }
    }

    pub(crate) fn cycle(cycle: Cycle) -> Self {
        Self {
            kind: ErrorKind::Cycle(CycleError {
                cycle,
                bomb: DropBomb::new("TODO"),
            }),
        }
    }

    pub(crate) fn into_cycle(self) -> std::result::Result<Cycle, Self> {
        match self.kind {
            ErrorKind::Cycle(cycle) => Ok(cycle.take_cycle()),
            _ => Err(self),
        }
    }
}

impl From<CycleError> for Error {
    fn from(value: CycleError) -> Self {
        Self {
            kind: ErrorKind::Cycle(value),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
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
    Cancelled(Cancelled),
}

#[derive(Debug)]
pub(crate) struct CycleError {
    cycle: Cycle,
    bomb: DropBomb,
}

impl CycleError {
    pub(crate) fn take_cycle(mut self) -> Cycle {
        self.bomb.defuse();
        self.cycle
    }
}

// FIXME implement drop for Cancelled.

/// A panic payload indicating that execution of a salsa query was cancelled.
///
/// This can occur for a few reasons:
/// *
/// *
/// *
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum Cancelled {
    /// The query was operating on revision R, but there is a pending write to move to revision R+1.
    #[non_exhaustive]
    PendingWrite,

    /// The query was blocked on another thread, and that thread panicked.
    #[non_exhaustive]
    PropagatedPanic,
}

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = match self {
            Cancelled::PendingWrite => "pending write",
            Cancelled::PropagatedPanic => "propagated panic",
        };
        f.write_str("cancelled because of ")?;
        f.write_str(why)
    }
}
