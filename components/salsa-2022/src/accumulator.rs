use crate::{
    cycle::CycleRecoveryStrategy,
    hash::FxDashMap,
    ingredient::{Ingredient, MutIngredient},
    key::DependencyIndex,
    runtime::{local_state::QueryOrigin, StampedValue},
    storage::HasJar,
    DatabaseKeyIndex, Durability, IngredientIndex, Revision, Runtime,
};

pub trait Accumulator {
    type Data: Clone;
    type Jar;

    fn accumulator_ingredient<'db, Db>(db: &'db Db) -> &'db AccumulatorIngredient<Self::Data>
    where
        Db: ?Sized + HasJar<Self::Jar>;
}

pub struct AccumulatorIngredient<Data: Clone> {
    map: FxDashMap<DatabaseKeyIndex, StampedValue<Vec<Data>>>,
}

impl<Data: Clone> AccumulatorIngredient<Data> {
    pub fn new(_index: IngredientIndex) -> Self {
        Self {
            map: FxDashMap::default(),
        }
    }

    pub fn push(&self, runtime: &Runtime, value: Data) {
        let (active_query, active_inputs) = match runtime.active_query() {
            Some(pair) => pair,
            None => {
                panic!("cannot accumulate values outside of an active query")
            }
        };

        let mut stamped_value = self.map.entry(active_query).or_insert(StampedValue {
            value: vec![],
            durability: Durability::MAX,
            changed_at: Revision::start(),
        });

        stamped_value.value.push(value);
        stamped_value
            .value_mut()
            .merge_revision_info(&active_inputs);
    }

    pub(crate) fn produced_by(
        &self,
        runtime: &Runtime,
        query: DatabaseKeyIndex,
        output: &mut Vec<Data>,
    ) {
        if let Some(v) = self.map.get(&query) {
            let StampedValue {
                value,
                durability,
                changed_at,
            } = v.value();
            runtime.report_tracked_read(query.into(), *durability, *changed_at);
            output.extend(value.iter().cloned());
        }
    }
}

impl<DB: ?Sized, Data> Ingredient<DB> for AccumulatorIngredient<Data>
where
    Data: Clone,
{
    fn maybe_changed_after(&self, _db: &DB, _input: DependencyIndex, _revision: Revision) -> bool {
        panic!("nothing should ever depend on an accumulator directly")
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn remove_stale_output(&self, executor: DatabaseKeyIndex, stale_output_key: Option<crate::Id>) {
        // FIXME
        drop((executor, stale_output_key));
    }
}

impl<DB: ?Sized, Data> MutIngredient<DB> for AccumulatorIngredient<Data>
where
    Data: Clone,
{
    fn reset_for_new_revision(&mut self) {
        // FIXME: We could certainly drop things here if we knew which ones
        // to drop. There's a fixed point algorithm we could be doing.
    }
}
