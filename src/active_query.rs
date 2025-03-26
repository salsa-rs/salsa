use std::ops::Not;
use std::sync::atomic::AtomicBool;
use std::{mem, ops};

use super::zalsa_local::{QueryEdges, QueryOrigin, QueryRevisions};
use crate::accumulator::accumulated_map::AtomicInputAccumulatedValues;
use crate::runtime::Stamp;
use crate::tracked_struct::{DisambiguatorMap, IdentityHash, IdentityMap};
use crate::zalsa_local::QueryEdge;
use crate::{
    accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues},
    cycle::CycleHeads,
    durability::Durability,
    hash::FxIndexSet,
    key::DatabaseKeyIndex,
    tracked_struct::Disambiguator,
    Revision,
};
use crate::{Accumulator, IngredientIndex};

#[derive(Debug)]
pub(crate) struct ActiveQuery {
    /// What query is executing
    pub(crate) database_key_index: DatabaseKeyIndex,

    /// Minimum durability of inputs observed so far.
    durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    changed_at: Revision,

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
    accumulated: AccumulatedMap,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any accumulated values.
    accumulated_inputs: InputAccumulatedValues,

    /// Provisional cycle results that this query depends on.
    cycle_heads: CycleHeads,
}

impl ActiveQuery {
    pub(super) fn add_read(
        &mut self,
        input: DatabaseKeyIndex,
        durability: Durability,
        revision: Revision,
        accumulated: InputAccumulatedValues,
        cycle_heads: &CycleHeads,
    ) {
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
        self.input_outputs.insert(QueryEdge::Input(input));
        self.accumulated_inputs |= accumulated;
        self.cycle_heads.extend(cycle_heads);
    }

    pub(super) fn add_read_simple(
        &mut self,
        input: DatabaseKeyIndex,
        durability: Durability,
        revision: Revision,
    ) {
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
        self.input_outputs.insert(QueryEdge::Input(input));
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

    pub(super) fn accumulate(&mut self, index: IngredientIndex, value: impl Accumulator) {
        self.accumulated.accumulate(index, value);
    }

    /// Adds a key to our list of outputs.
    pub(super) fn add_output(&mut self, key: DatabaseKeyIndex) {
        self.input_outputs.insert(QueryEdge::Output(key));
    }

    /// True if the given key was output by this query.
    pub(super) fn is_output(&self, key: DatabaseKeyIndex) -> bool {
        self.input_outputs.contains(&QueryEdge::Output(key))
    }

    pub(super) fn disambiguate(&mut self, key: IdentityHash) -> Disambiguator {
        self.disambiguator_map.disambiguate(key)
    }

    pub(super) fn stamp(&self) -> Stamp {
        Stamp {
            value: (),
            durability: self.durability,
            changed_at: self.changed_at,
        }
    }
}

impl ActiveQuery {
    fn new(database_key_index: DatabaseKeyIndex) -> Self {
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

    fn top_into_revisions(&mut self) -> QueryRevisions {
        let &mut Self {
            database_key_index: _,
            durability,
            changed_at,
            ref mut input_outputs,
            untracked_read,
            ref mut disambiguator_map,
            ref mut tracked_struct_ids,
            ref mut accumulated,
            accumulated_inputs,
            ref mut cycle_heads,
        } = self;

        let edges = QueryEdges::new(input_outputs.drain(..));
        let origin = if untracked_read {
            QueryOrigin::DerivedUntracked(edges)
        } else {
            QueryOrigin::Derived(edges)
        };
        disambiguator_map.clear();
        let accumulated = accumulated
            .is_empty()
            .not()
            .then(|| Box::new(mem::take(accumulated)));
        let tracked_struct_ids = tracked_struct_ids
            .is_empty()
            .not()
            .then(|| Box::new(mem::take(tracked_struct_ids)));
        let accumulated_inputs = AtomicInputAccumulatedValues::new(accumulated_inputs);
        let cycle_heads = mem::take(cycle_heads);
        QueryRevisions {
            changed_at,
            durability,
            origin,
            tracked_struct_ids,
            accumulated_inputs,
            accumulated,
            verified_final: AtomicBool::new(cycle_heads.is_empty()),
            cycle_heads,
        }
    }

    fn clear(&mut self) {
        let Self {
            database_key_index: _,
            durability: _,
            changed_at: _,
            input_outputs,
            untracked_read: _,
            disambiguator_map,
            tracked_struct_ids,
            accumulated,
            accumulated_inputs: _,
            cycle_heads,
        } = self;
        input_outputs.clear();
        disambiguator_map.clear();
        tracked_struct_ids.clear();
        accumulated.clear();
        *cycle_heads = Default::default();
    }

    fn reset_for(&mut self, new_database_key_index: DatabaseKeyIndex) {
        let Self {
            database_key_index,
            durability,
            changed_at,
            input_outputs,
            untracked_read,
            disambiguator_map,
            tracked_struct_ids,
            accumulated,
            accumulated_inputs,
            cycle_heads,
        } = self;
        *database_key_index = new_database_key_index;
        *durability = Durability::MAX;
        *changed_at = Revision::start();
        *untracked_read = false;
        *accumulated_inputs = Default::default();
        debug_assert!(
            input_outputs.is_empty(),
            "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
        );
        debug_assert!(
            disambiguator_map.is_empty(),
            "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
        );
        debug_assert!(
            tracked_struct_ids.is_empty(),
            "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
        );
        debug_assert!(
            cycle_heads.is_empty(),
            "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
        );
        debug_assert!(
            accumulated.is_empty(),
            "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
        );
    }
}

#[derive(Debug, Default)]
pub(crate) struct QueryStack {
    stack: Vec<ActiveQuery>,
    len: usize,
}

impl ops::Deref for QueryStack {
    type Target = [ActiveQuery];

    fn deref(&self) -> &Self::Target {
        &self.stack[..self.len]
    }
}

impl ops::DerefMut for QueryStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stack[..self.len]
    }
}

impl QueryStack {
    pub(crate) fn push_new_query(&mut self, database_key_index: DatabaseKeyIndex) {
        if self.len < self.stack.len() {
            self.stack[self.len].reset_for(database_key_index);
        } else {
            self.stack.push(ActiveQuery::new(database_key_index));
        }
        self.len += 1;
    }

    #[cfg(debug_assertions)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn pop_into_revisions(
        &mut self,
        key: DatabaseKeyIndex,
        #[cfg(debug_assertions)] push_len: usize,
    ) -> QueryRevisions {
        #[cfg(debug_assertions)]
        assert_eq!(push_len, self.len(), "unbalanced push/pop");
        debug_assert_ne!(self.len, 0, "too many pops");
        self.len -= 1;
        debug_assert_eq!(
            self.stack[self.len].database_key_index, key,
            "unbalanced push/pop"
        );
        self.stack[self.len].top_into_revisions()
    }

    pub(crate) fn pop(&mut self, key: DatabaseKeyIndex, #[cfg(debug_assertions)] push_len: usize) {
        #[cfg(debug_assertions)]
        assert_eq!(push_len, self.len(), "unbalanced push/pop");
        debug_assert_ne!(self.len, 0, "too many pops");
        self.len -= 1;
        debug_assert_eq!(
            self.stack[self.len].database_key_index, key,
            "unbalanced push/pop"
        );
        self.stack[self.len].clear()
    }
}
