use crate::{accumulator, storage::DatabaseGen, Database, Id};

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

        let database_key_index = self.database_key_index(key);
        accumulator.produced_by(runtime, database_key_index, &mut output);

        if let Some(origin) = self.origin(key) {
            for input in origin.inputs() {
                if let Ok(input) = input.try_into() {
                    accumulator.produced_by(runtime, input, &mut output);
                }
            }
        }

        output
    }
}
