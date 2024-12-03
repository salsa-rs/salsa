//! Basic test of accumulator functionality.

use std::{
    any::Any,
    fmt::{self, Debug},
    marker::PhantomData,
};

use accumulated::Accumulated;
use accumulated::AnyAccumulated;
use accumulated_map::AccumulatedMap;

use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, Ingredient, Jar},
    plumbing::JarAux,
    zalsa::IngredientIndex,
    zalsa_local::QueryOrigin,
    Database, DatabaseKeyIndex, Id, Revision,
};

mod accumulated;
pub(crate) mod accumulated_map;

/// Trait implemented on the struct that user annotated with `#[salsa::accumulator]`.
/// The `Self` type is therefore the types to be accumulated.
pub trait Accumulator: Clone + Debug + Send + Sync + Any + Sized {
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
    fn create_ingredients(
        &self,
        _aux: &dyn JarAux,
        first_index: IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(<IngredientImpl<A>>::new(first_index))]
    }
}

pub struct IngredientImpl<A: Accumulator> {
    index: IngredientIndex,
    phantom: PhantomData<Accumulated<A>>,
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
            index,
            phantom: PhantomData,
        }
    }

    pub fn push(&self, db: &dyn Database, value: A) {
        let zalsa_local = db.zalsa_local();
        if let Err(()) = zalsa_local.accumulate(self.index, value) {
            panic!("cannot accumulate values outside of an active tracked function");
        }
    }

    pub fn index(&self) -> IngredientIndex {
        self.index
    }
}

impl<A: Accumulator> Ingredient for IngredientImpl<A> {
    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    fn maybe_changed_after(&self, _db: &dyn Database, _input: Id, _revision: Revision) -> bool {
        panic!("nothing should ever depend on an accumulator directly")
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _db: &dyn Database, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        _executor: DatabaseKeyIndex,
        _output_key: Id,
    ) {
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        _executor: DatabaseKeyIndex,
        _stale_output_key: Id,
    ) {
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        panic!("unexpected reset on accumulator")
    }

    fn fmt_index(&self, index: Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(A::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        A::DEBUG_NAME
    }

    fn accumulated(&self, _db: &dyn Database, _key_index: Id) -> Option<&AccumulatedMap> {
        None
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
