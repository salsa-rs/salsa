use crate::{runtime::RuntimeId, DatabaseKeyIndex};
use std::fmt;

/// The `Event` struct identifies various notable things that can
/// occur during salsa execution. Instances of this struct are given
/// to `salsa_event`.
pub struct Event {
    /// The id of the snapshot that triggered the event.  Usually
    /// 1-to-1 with a thread, as well.
    pub runtime_id: RuntimeId,

    /// What sort of event was it.
    pub kind: EventKind,
}

// impl Event {
//     /// Returns a type that gives a user-readable debug output.
//     /// Use like `println!("{:?}", index.debug(db))`.
//     pub fn debug<'me, D: ?Sized>(&'me self, db: &'me D) -> impl std::fmt::Debug + 'me
//     where
//         D: plumbing::DatabaseOps,
//     {
//         EventDebug { event: self, db }
//     }
// }

impl fmt::Debug for Event {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind)
            .finish()
    }
}

// struct EventDebug<'me, D: ?Sized>
// where
//     D: plumbing::DatabaseOps,
// {
//     event: &'me Event,
//     db: &'me D,
// }
//
// impl<'me, D: ?Sized> fmt::Debug for EventDebug<'me, D>
// where
//     D: plumbing::DatabaseOps,
// {
//     fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
//         fmt.debug_struct("Event")
//             .field("runtime_id", &self.event.runtime_id)
//             .field("kind", &self.event.kind.debug(self.db))
//             .finish()
//     }
// }

/// An enum identifying the various kinds of events that can occur.
pub enum EventKind {
    /// Occurs when we found that all inputs to a memoized value are
    /// up-to-date and hence the value can be re-used without
    /// executing the closure.
    ///
    /// Executes before the "re-used" value is returned.
    DidValidateMemoizedValue {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DatabaseKeyIndex,
    },

    /// Indicates that another thread (with id `other_runtime_id`) is processing the
    /// given query (`database_key`), so we will block until they
    /// finish.
    ///
    /// Executes after we have registered with the other thread but
    /// before they have answered us.
    ///
    /// (NB: you can find the `id` of the current thread via the
    /// `salsa_runtime`)
    WillBlockOn {
        /// The id of the runtime we will block on.
        other_runtime_id: RuntimeId,

        /// The database-key for the affected value. Implements `Debug`.
        database_key: DatabaseKeyIndex,
    },

    /// Indicates that the function for this query will be executed.
    /// This is either because it has never executed before or because
    /// its inputs may be out of date.
    WillExecute {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DatabaseKeyIndex,
    },

    /// Indicates that `unwind_if_cancelled` was called and salsa will check if
    /// the current revision has been cancelled.
    WillCheckCancellation,
}

// impl EventKind {
//     /// Returns -a type that gives a user-readable debug output.
//     /// Use like `println!("{:?}", index.debug(db))`.
//     pub fn debug<'me, D: ?Sized>(&'me self, db: &'me D) -> impl std::fmt::Debug + 'me
//     where
//         D: plumbing::DatabaseOps,
//     {
//         EventKindDebug { kind: self, db }
//     }
// }

impl fmt::Debug for EventKind {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventKind::DidValidateMemoizedValue { database_key } => fmt
                .debug_struct("DidValidateMemoizedValue")
                .field("database_key", database_key)
                .finish(),
            EventKind::WillBlockOn {
                other_runtime_id,
                database_key,
            } => fmt
                .debug_struct("WillBlockOn")
                .field("other_runtime_id", other_runtime_id)
                .field("database_key", database_key)
                .finish(),
            EventKind::WillExecute { database_key } => fmt
                .debug_struct("WillExecute")
                .field("database_key", database_key)
                .finish(),
            EventKind::WillCheckCancellation => fmt.debug_struct("WillCheckCancellation").finish(),
        }
    }
}

// struct EventKindDebug<'me, D: ?Sized>
// where
//     D: plumbing::DatabaseOps,
// {
//     kind: &'me EventKind,
//     db: &'me D,
// }

// impl<'me, D: ?Sized> fmt::Debug for EventKindDebug<'me, D>
// where
//     D: plumbing::DatabaseOps,
// {
//     fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
//         match self.kind {
//             EventKind::DidValidateMemoizedValue { database_key } => fmt
//                 .debug_struct("DidValidateMemoizedValue")
//                 .field("database_key", &database_key.debug(self.db))
//                 .finish(),
//             EventKind::WillBlockOn {
//                 other_runtime_id,
//                 database_key,
//             } => fmt
//                 .debug_struct("WillBlockOn")
//                 .field("other_runtime_id", &other_runtime_id)
//                 .field("database_key", &database_key.debug(self.db))
//                 .finish(),
//             EventKind::WillExecute { database_key } => fmt
//                 .debug_struct("WillExecute")
//                 .field("database_key", &database_key.debug(self.db))
//                 .finish(),
//             EventKind::WillCheckCancellation => fmt.debug_struct("WillCheckCancellation").finish(),
//         }
//     }
// }
