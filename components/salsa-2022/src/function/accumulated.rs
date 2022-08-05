use crate::{
    hash::FxHashSet,
    key::DependencyIndex,
    runtime::local_state::QueryInputs,
    storage::{HasJar, HasJarsDyn},
    Database, DatabaseKeyIndex,
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
        let runtime = db.salsa_runtime();
        let mut result = vec![];
        let accumulator_ingredient = A::accumulator_ingredient(db);
        let mut stack = Stack::new(self.database_key_index(key));
        while let Some(input) = stack.pop() {
            accumulator_ingredient.produced_by(runtime, input, &mut result);
            stack.extend(db.inputs(input));
        }
        result
    }
}

struct Stack {
    v: Vec<DatabaseKeyIndex>,
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

    fn extend(&mut self, inputs: Option<QueryInputs>) {
        let inputs = match inputs {
            None => return,
            Some(v) => v,
        };

        for DependencyIndex {
            ingredient_index,
            key_index,
        } in inputs.tracked.iter().copied()
        {
            if let Some(key_index) = key_index {
                let i = DatabaseKeyIndex {
                    ingredient_index,
                    key_index,
                };
                if self.s.insert(i) {
                    self.v.push(i);
                }
            }
        }
    }
}
