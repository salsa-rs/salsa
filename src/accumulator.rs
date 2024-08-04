//! Basic test of accumulator functionality.

use std::{
    fmt::{self, Debug},
    marker::PhantomData,
};

use crate::{
    cycle::CycleRecoveryStrategy,
    hash::FxDashMap,
    ingredient::{fmt_index, Ingredient, Jar},
    key::DependencyIndex,
    zalsa::IngredientIndex,
    zalsa_local::{QueryOrigin, ZalsaLocal},
    Database, DatabaseKeyIndex, Event, EventKind, Id, Revision,
};

pub trait Accumulator: Clone + Debug + Send + Sync + 'static + Sized {
    const DEBUG_NAME: &'static str;

    /// Accumulate an instance of this in the database for later retrieval.
    fn accumulate<Db>(self, db: &Db)
    where
        Db: ?Sized + Database;
}

pub struct JarImpl<A: Accumulator> {
    phantom: PhantomData<A>,
}

impl<A: Accumulator> Default for JarImpl<A> {
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}

impl<A: Accumulator> Jar for JarImpl<A> {
    fn create_ingredients(&self, first_index: IngredientIndex) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(<IngredientImpl<A>>::new(first_index))]
    }
}

pub struct IngredientImpl<A: Accumulator> {
    index: IngredientIndex,
    map: FxDashMap<DatabaseKeyIndex, AccumulatedValues<A>>,
}

struct AccumulatedValues<A> {
    produced_at: Revision,
    values: Vec<A>,
}

impl<A: Accumulator> IngredientImpl<A> {
    /// Find the accumulator ingrediate for `A` in the database, if any.
    pub fn from_db<Db>(db: &Db) -> Option<&Self>
    where
        Db: ?Sized + Database,
    {
        let jar: JarImpl<A> = Default::default();
        let zalsa = db.zalsa();
        let index = zalsa.add_or_lookup_jar_by_type(&jar);
        let ingredient = zalsa.lookup_ingredient(index).assert_type::<Self>();
        Some(ingredient)
    }

    pub fn new(index: IngredientIndex) -> Self {
        Self {
            map: FxDashMap::default(),
            index,
        }
    }

    fn dependency_index(&self) -> DependencyIndex {
        DependencyIndex {
            ingredient_index: self.index,
            key_index: None,
        }
    }

    pub fn push(&self, db: &dyn crate::Database, value: A) {
        let state = db.zalsa_local();
        let current_revision = db.zalsa().current_revision();
        let (active_query, _) = match state.active_query() {
            Some(pair) => pair,
            None => {
                panic!("cannot accumulate values outside of an active query")
            }
        };

        let mut accumulated_values = self.map.entry(active_query).or_insert(AccumulatedValues {
            values: vec![],
            produced_at: current_revision,
        });

        // When we call `push' in a query, we will add the accumulator to the output of the query.
        // If we find here that this accumulator is not the output of the query,
        // we can say that the accumulated values we stored for this query is out of date.
        if !state.is_output_of_active_query(self.dependency_index()) {
            accumulated_values.values.truncate(0);
            accumulated_values.produced_at = current_revision;
        }

        state.add_output(self.dependency_index());
        accumulated_values.values.push(value);
    }

    pub(crate) fn produced_by(
        &self,
        current_revision: Revision,
        local_state: &ZalsaLocal,
        query: DatabaseKeyIndex,
        output: &mut Vec<A>,
    ) {
        if let Some(v) = self.map.get(&query) {
            // FIXME: We don't currently have a good way to identify the value that was read.
            // You can't report is as a tracked read of `query`, because the return value of query is not being read here --
            // instead it is the set of values accumuated by `query`.
            local_state.report_untracked_read(current_revision);

            let AccumulatedValues {
                values,
                produced_at,
            } = v.value();

            if *produced_at == current_revision {
                output.extend(values.iter().cloned());
            }
        }
    }
}

impl<A: Accumulator> Ingredient for IngredientImpl<A> {
    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        _input: Option<Id>,
        _revision: Revision,
    ) -> bool {
        panic!("nothing should ever depend on an accumulator directly")
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        db: &dyn Database,
        executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        assert!(output_key.is_none());
        let current_revision = db.zalsa().current_revision();
        if let Some(mut v) = self.map.get_mut(&executor) {
            // The value is still valid in the new revision.
            v.produced_at = current_revision;
        }
    }

    fn remove_stale_output(
        &self,
        db: &dyn Database,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        assert!(stale_output_key.is_none());
        if self.map.remove(&executor).is_some() {
            db.salsa_event(&|| Event {
                thread_id: std::thread::current().id(),
                kind: EventKind::DidDiscardAccumulated {
                    executor_key: executor,
                    accumulator: self.dependency_index(),
                },
            })
        }
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        panic!("unexpected reset on accumulator")
    }

    fn salsa_struct_deleted(&self, _db: &dyn Database, _id: crate::Id) {
        panic!("unexpected call: accumulator is not registered as a dependent fn");
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(A::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        A::DEBUG_NAME
    }
}

impl<A> std::fmt::Debug for IngredientImpl<A>
where
    A: Accumulator,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.index)
            .finish()
    }
}
