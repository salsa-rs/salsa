use crate::{
    debug::DebugWithDb, key::DatabaseKeyIndex, key::DependencyIndex, runtime::RuntimeId, Database,
};
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

impl fmt::Debug for Event {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind)
            .finish()
    }
}

impl<Db> DebugWithDb<Db> for Event
where
    Db: ?Sized + Database,
{
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        db: &Db,
        include_all_fields: bool,
    ) -> std::fmt::Result {
        f.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind.debug_with(db, include_all_fields))
            .finish()
    }
}

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
    /// `runtime`)
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

    /// Discovered that a query used to output a given output but no longer does.
    WillDiscardStaleOutput {
        /// Key for the query that is executing and which no longer outputs the given value.
        execute_key: DatabaseKeyIndex,

        /// Key for the query that is no longer output
        output_key: DependencyIndex,
    },

    /// Tracked structs or memoized data were discarded (freed).
    DidDiscard {
        /// Value being discarded.
        key: DatabaseKeyIndex,
    },

    /// Discarded accumulated data from a given fn
    DidDiscardAccumulated {
        /// The key of the fn that accumulated results
        executor_key: DatabaseKeyIndex,

        /// Accumulator that was accumulated into
        accumulator: DependencyIndex,
    },
}

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
            EventKind::WillDiscardStaleOutput {
                execute_key,
                output_key,
            } => fmt
                .debug_struct("WillDiscardStaleOutput")
                .field("execute_key", &execute_key)
                .field("output_key", &output_key)
                .finish(),
            EventKind::DidDiscard { key } => {
                fmt.debug_struct("DidDiscard").field("key", &key).finish()
            }
            EventKind::DidDiscardAccumulated {
                executor_key,
                accumulator,
            } => fmt
                .debug_struct("DidDiscardAccumulated")
                .field("executor_key", executor_key)
                .field("accumulator", accumulator)
                .finish(),
        }
    }
}

impl<Db> DebugWithDb<Db> for EventKind
where
    Db: ?Sized + Database,
{
    fn fmt(
        &self,
        fmt: &mut std::fmt::Formatter<'_>,
        db: &Db,
        include_all_fields: bool,
    ) -> std::fmt::Result {
        match self {
            EventKind::DidValidateMemoizedValue { database_key } => fmt
                .debug_struct("DidValidateMemoizedValue")
                .field(
                    "database_key",
                    &database_key.debug_with(db, include_all_fields),
                )
                .finish(),
            EventKind::WillBlockOn {
                other_runtime_id,
                database_key,
            } => fmt
                .debug_struct("WillBlockOn")
                .field("other_runtime_id", other_runtime_id)
                .field(
                    "database_key",
                    &database_key.debug_with(db, include_all_fields),
                )
                .finish(),
            EventKind::WillExecute { database_key } => fmt
                .debug_struct("WillExecute")
                .field(
                    "database_key",
                    &database_key.debug_with(db, include_all_fields),
                )
                .finish(),
            EventKind::WillCheckCancellation => fmt.debug_struct("WillCheckCancellation").finish(),
            EventKind::WillDiscardStaleOutput {
                execute_key,
                output_key,
            } => fmt
                .debug_struct("WillDiscardStaleOutput")
                .field(
                    "execute_key",
                    &execute_key.debug_with(db, include_all_fields),
                )
                .field("output_key", &output_key.debug_with(db, include_all_fields))
                .finish(),
            EventKind::DidDiscard { key } => fmt
                .debug_struct("DidDiscard")
                .field("key", &key.debug_with(db, include_all_fields))
                .finish(),
            EventKind::DidDiscardAccumulated {
                executor_key,
                accumulator,
            } => fmt
                .debug_struct("DidDiscardAccumulated")
                .field(
                    "executor_key",
                    &executor_key.debug_with(db, include_all_fields),
                )
                .field(
                    "accumulator",
                    &accumulator.debug_with(db, include_all_fields),
                )
                .finish(),
        }
    }
}
