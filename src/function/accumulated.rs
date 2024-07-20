use std::collections::HashSet;

use crate::{accumulator, storage::DatabaseGen, DatabaseKeyIndex, Id};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Helper used by `accumulate` functions. Computes the results accumulated by `database_key_index`
    /// and its inputs.
    pub fn accumulated_by<A>(&self, db: &C::DbView, key: Id) -> Vec<A>
    where
        A: accumulator::Accumulator,
    {
        let Some(accumulator) = <accumulator::IngredientImpl<A>>::from_db(db) else {
            return vec![];
        };
        let runtime = db.runtime();
        let mut output = vec![];

        // First ensure the result is up to date
        self.fetch(db, key);

        // Recursively accumulate outputs from children
        self.database_key_index(key).traverse_children::<C, _>(
            db,
            &mut |query| accumulator.produced_by(runtime, query, &mut output),
            &mut HashSet::new(),
        );

        output
    }
}

impl DatabaseKeyIndex {
    pub fn traverse_children<C, F>(
        &self,
        db: &C::DbView,
        handler: &mut F,
        visited: &mut HashSet<DatabaseKeyIndex>,
    ) where
        C: Configuration,
        F: (FnMut(DatabaseKeyIndex)),
    {
        handler(*self);
        visited.insert(*self);

        let origin = db
            .lookup_ingredient(self.ingredient_index)
            .origin(self.key_index);

        if let Some(origin) = origin {
            for input in origin.inputs() {
                if let Ok(input) = TryInto::<DatabaseKeyIndex>::try_into(input) {
                    if !visited.contains(&input) {
                        input.traverse_children::<C, F>(db, handler, visited);
                    }
                }
            }
        }
    }
}
