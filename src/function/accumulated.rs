use crate::{accumulator, hash::FxHashSet, local_state, Database, DatabaseKeyIndex, Id};

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
        local_state::attach(db, |local_state| {
            let zalsa = db.zalsa();
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
                    accumulator.produced_by(current_revision, local_state, k, &mut output);

                    let origin = zalsa
                        .lookup_ingredient(k.ingredient_index)
                        .origin(k.key_index);
                    let inputs = origin.iter().flat_map(|origin| origin.inputs());
                    // Careful: we want to push in execution order, so reverse order to
                    // ensure the first child that was executed will be the first child popped
                    // from the stack.
                    stack.extend(
                        inputs
                            .flat_map(|input| {
                                TryInto::<DatabaseKeyIndex>::try_into(input).into_iter()
                            })
                            .rev(),
                    );
                }
            }

            output
        })
    }
}
