use crate::{accumulator, hash::FxHashSet, zalsa::ZalsaDatabase, DatabaseKeyIndex, Id};

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
        let zalsa = db.zalsa();
        let zalsa_local = db.zalsa_local();
        let current_revision = zalsa.current_revision();

        let Some(accumulator) = <accumulator::IngredientImpl<A>>::from_db(db) else {
            return vec![];
        };
        let mut output = vec![];

        // First ensure the result is up to date
        self.fetch(db, key);

        let db_key = self.database_key_index(key);
        let mut visited: FxHashSet<DatabaseKeyIndex> = FxHashSet::default();
        let mut stack: Vec<DatabaseKeyIndex> = vec![db_key];

        while let Some(k) = stack.pop() {
            if visited.insert(k) {
                accumulator.produced_by(current_revision, zalsa_local, k, &mut output);

                let origin = zalsa
                    .lookup_ingredient(k.ingredient_index)
                    .origin(k.key_index);
                let inputs = origin.iter().flat_map(|origin| origin.inputs());
                // Careful: we want to push in execution order, so reverse order to
                // ensure the first child that was executed will be the first child popped
                // from the stack.
                stack.extend(
                    inputs
                        .flat_map(|input| TryInto::<DatabaseKeyIndex>::try_into(input).into_iter())
                        .rev(),
                );
            }
        }

        output
    }
}
