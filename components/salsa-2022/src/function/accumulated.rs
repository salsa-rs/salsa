use crate::{
    hash::FxHashSet,
    runtime::local_state::QueryOrigin,
    storage::{HasJar, HasJarsDyn},
    DatabaseKeyIndex,
};

use super::{Configuration, DynDb, FunctionIngredient};
use crate::accumulator::Accumulator;

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Returns all the values accumulated into `accumulator` by this query and its
    /// transitive inputs.
    pub fn accumulated<'db, A>(&self, db: &DynDb<'db, C>, key: C::Key) -> Vec<A::Data>
    where
        DynDb<'db, C>: HasJar<A::Jar>,
        A: Accumulator,
    {
        // To start, ensure that the value is up to date:
        self.fetch(db, key);

        // Now walk over all the things that the value depended on
        // and find the values they accumulated into the given
        // accumulator:
        let runtime = db.runtime();
        let mut result = vec![];
        let accumulator_ingredient = A::accumulator_ingredient(db);
        let mut stack = Stack::new(self.database_key_index(key));
        while let Some(input) = stack.pop() {
            accumulator_ingredient.produced_by(runtime, input, &mut result);
            stack.extend(db.origin(input));
        }
        result
    }
}

/// The stack is used to execute a DFS across all the queries
/// that were transitively executed by some given start query.
/// When we visit a query Q0, we look at its dependencies Q1...Qn,
/// and if they have not already been visited, we push them on the stack.
struct Stack {
    /// Stack of queries left to visit.
    v: Vec<DatabaseKeyIndex>,

    /// Set of all queries we've seen.
    s: FxHashSet<DatabaseKeyIndex>,
}

impl Stack {
    fn new(start: DatabaseKeyIndex) -> Self {
        Self {
            v: vec![start],
            s: FxHashSet::default(),
        }
    }

    fn pop(&mut self) -> Option<DatabaseKeyIndex> {
        self.v.pop()
    }

    /// Extend the stack of queries with the dependencies from `origin`.
    fn extend(&mut self, origin: Option<QueryOrigin>) {
        match origin {
            None | Some(QueryOrigin::Assigned(_)) | Some(QueryOrigin::BaseInput) => {}
            Some(QueryOrigin::Derived(edges)) | Some(QueryOrigin::DerivedUntracked(edges)) => {
                for dependency_index in edges.inputs() {
                    if let Ok(i) = DatabaseKeyIndex::try_from(dependency_index) {
                        if self.s.insert(i) {
                            self.v.push(i)
                        }
                    }
                }
            }
        }
    }
}
