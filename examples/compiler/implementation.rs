use crate::compiler::CompilerDatabase;
use crate::interner::InternerDatabase;

/// Our "database" will be threaded through our application (though
/// 99% of the application only interacts with it through traits and
/// never knows its real name). It contains all the values for all of
/// our memoized queries and encapsulates **all mutable state that
/// persists longer than a single query execution.**
///
/// Databases can contain whatever you want them to, but salsa
/// requires you to add a `salsa::Runtime` member. Note
/// though: you should be very careful if adding shared, mutable state
/// to your context (e.g., a shared counter or some such thing). If
/// mutations to that shared state affect the results of your queries,
/// that's going to mess up the incremental results.
#[salsa::database(InternerDatabase, CompilerDatabase)]
#[derive(Default)]
pub struct DatabaseImpl {
    storage: salsa::Storage<DatabaseImpl>,
}

/// This impl tells salsa where to find the salsa runtime.
impl salsa::Database for DatabaseImpl {}
