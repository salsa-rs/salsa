use std::ops::Not;

use super::zalsa_local::{QueryEdges, QueryOrigin, QueryRevisions};
use crate::accumulator::accumulated_map::AtomicInputAccumulatedValues;
use crate::key::OutputDependencyIndex;
use crate::tracked_struct::{DisambiguatorMap, IdentityHash, IdentityMap};
use crate::zalsa_local::QueryEdge;
use crate::{
    accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues},
    cycle::CycleHeads,
    durability::Durability,
    hash::FxIndexSet,
    key::{DatabaseKeyIndex, InputDependencyIndex},
    tracked_struct::Disambiguator,
    Revision,
};

#[derive(Debug)]
pub(crate) struct ActiveQuery {
    /// What query is executing
    pub(crate) database_key_index: DatabaseKeyIndex,

    /// Minimum durability of inputs observed so far.
    pub(crate) durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    pub(crate) changed_at: Revision,

    /// Inputs: Set of subqueries that were accessed thus far.
    /// Outputs: Tracks values written by this query. Could be...
    ///
    /// * tracked structs created
    /// * invocations of `specify`
    /// * accumulators pushed to
    input_outputs: FxIndexSet<QueryEdge>,

    /// True if there was an untracked read.
    untracked_read: bool,

    /// When new tracked structs are created, their data is hashed, and the resulting
    /// hash is added to this map. If it is not present, then the disambiguator is 0.
    /// Otherwise it is 1 more than the current value (which is incremented).
    ///
    /// This table starts empty as the query begins and is gradually populated.
    /// Note that if a query executes in 2 different revisions but creates the same
    /// set of tracked structs, they will get the same disambiguator values.
    disambiguator_map: DisambiguatorMap,

    /// Map from tracked struct keys (which include the hash + disambiguator) to their
    /// final id.
    pub(crate) tracked_struct_ids: IdentityMap,

    /// Stores the values accumulated to the given ingredient.
    /// The type of accumulated value is erased but known to the ingredient.
    pub(crate) accumulated: AccumulatedMap,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any accumulated values.
    pub(super) accumulated_inputs: InputAccumulatedValues,

    /// Provisional cycle results that this query depends on.
    pub(crate) cycle_heads: CycleHeads,
}

impl ActiveQuery {
    pub(super) fn new(database_key_index: DatabaseKeyIndex) -> Self {
        ActiveQuery {
            database_key_index,
            durability: Durability::MAX,
            changed_at: Revision::start(),
            input_outputs: FxIndexSet::default(),
            untracked_read: false,
            disambiguator_map: Default::default(),
            tracked_struct_ids: Default::default(),
            accumulated: Default::default(),
            accumulated_inputs: Default::default(),
            cycle_heads: Default::default(),
        }
    }

    pub(super) fn add_read(
        &mut self,
        input: InputDependencyIndex,
        durability: Durability,
        revision: Revision,
        accumulated: InputAccumulatedValues,
        cycle_heads: &CycleHeads,
    ) {
        self.input_outputs.insert(QueryEdge::Input(input));
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
        self.accumulated_inputs |= accumulated;
        self.cycle_heads.extend(cycle_heads);
    }

    pub(super) fn add_untracked_read(&mut self, changed_at: Revision) {
        self.untracked_read = true;
        self.durability = Durability::MIN;
        self.changed_at = changed_at;
    }

    pub(super) fn add_synthetic_read(&mut self, durability: Durability, revision: Revision) {
        self.untracked_read = true;
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
    }

    /// Adds a key to our list of outputs.
    pub(super) fn add_output(&mut self, key: OutputDependencyIndex) {
        self.input_outputs.insert(QueryEdge::Output(key));
    }

    /// True if the given key was output by this query.
    pub(super) fn is_output(&self, key: OutputDependencyIndex) -> bool {
        self.input_outputs.contains(&QueryEdge::Output(key))
    }

    pub(crate) fn into_revisions(self) -> QueryRevisions {
        let edges = QueryEdges::new(self.input_outputs);
        let origin = if self.untracked_read {
            QueryOrigin::DerivedUntracked(edges)
        } else {
            QueryOrigin::Derived(edges)
        };
        let accumulated = self
            .accumulated
            .is_empty()
            .not()
            .then(|| Box::new(self.accumulated));
        QueryRevisions {
            changed_at: self.changed_at,
            origin,
            durability: self.durability,
            tracked_struct_ids: self.tracked_struct_ids,
            accumulated_inputs: AtomicInputAccumulatedValues::new(self.accumulated_inputs),
            accumulated,
            cycle_heads: self.cycle_heads,
        }
    }

    pub(super) fn disambiguate(&mut self, key: IdentityHash) -> Disambiguator {
        self.disambiguator_map.disambiguate(key)
    }
}
