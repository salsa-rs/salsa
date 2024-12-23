use rustc_hash::FxHashMap;

use super::zalsa_local::{EdgeKind, QueryEdges, QueryOrigin, QueryRevisions};
use crate::tracked_struct::IdentityHash;
use crate::{
    accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues},
    durability::Durability,
    hash::FxIndexSet,
    key::{DatabaseKeyIndex, DependencyIndex},
    tracked_struct::{Disambiguator, Identity},
    Cycle, Id, Revision,
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
    input_outputs: FxIndexSet<(EdgeKind, DependencyIndex)>,

    /// True if there was an untracked read.
    untracked_read: bool,

    /// Stores the entire cycle, if one is found and this query is part of it.
    pub(crate) cycle: Option<Cycle>,

    /// When new tracked structs are created, their data is hashed, and the resulting
    /// hash is added to this map. If it is not present, then the disambiguator is 0.
    /// Otherwise it is 1 more than the current value (which is incremented).
    ///
    /// This table starts empty as the query begins and is gradually populated.
    /// Note that if a query executes in 2 different revisions but creates the same
    /// set of tracked structs, they will get the same disambiguator values.
    disambiguator_map: FxHashMap<IdentityHash, Disambiguator>,

    /// Map from tracked struct keys (which include the hash + disambiguator) to their
    /// final id.
    pub(crate) tracked_struct_ids: FxHashMap<Identity, Id>,

    /// Stores the values accumulated to the given ingredient.
    /// The type of accumulated value is erased but known to the ingredient.
    pub(crate) accumulated: AccumulatedMap,
}

impl ActiveQuery {
    pub(super) fn new(database_key_index: DatabaseKeyIndex) -> Self {
        ActiveQuery {
            database_key_index,
            durability: Durability::MAX,
            changed_at: Revision::start(),
            input_outputs: FxIndexSet::default(),
            untracked_read: false,
            cycle: None,
            disambiguator_map: Default::default(),
            tracked_struct_ids: Default::default(),
            accumulated: Default::default(),
        }
    }

    pub(super) fn add_read(
        &mut self,
        input: DependencyIndex,
        durability: Durability,
        revision: Revision,
        accumulated: InputAccumulatedValues,
    ) {
        self.input_outputs.insert((EdgeKind::Input, input));
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
        self.accumulated.add_input(accumulated);
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
    pub(super) fn add_output(&mut self, key: DependencyIndex) {
        self.input_outputs.insert((EdgeKind::Output, key));
    }

    /// True if the given key was output by this query.
    pub(super) fn is_output(&self, key: DependencyIndex) -> bool {
        self.input_outputs.contains(&(EdgeKind::Output, key))
    }

    pub(crate) fn into_revisions(self) -> QueryRevisions {
        let input_outputs = if self.input_outputs.is_empty() {
            Box::default()
        } else {
            self.input_outputs.into_iter().collect()
        };

        let edges = QueryEdges::new(input_outputs);

        let origin = if self.untracked_read {
            QueryOrigin::DerivedUntracked(edges)
        } else {
            QueryOrigin::Derived(edges)
        };

        QueryRevisions {
            changed_at: self.changed_at,
            origin,
            durability: self.durability,
            tracked_struct_ids: self.tracked_struct_ids,
            accumulated: self.accumulated,
        }
    }

    /// Adds any dependencies from `other` into `self`.
    /// Used during cycle recovery, see [`Runtime::unblock_cycle_and_maybe_throw`].
    pub(super) fn add_from(&mut self, other: &ActiveQuery) {
        self.changed_at = self.changed_at.max(other.changed_at);
        self.durability = self.durability.min(other.durability);
        self.untracked_read |= other.untracked_read;
        self.input_outputs
            .extend(other.input_outputs.iter().copied());
    }

    /// Removes the participants in `cycle` from my dependencies.
    /// Used during cycle recovery, see [`Runtime::unblock_cycle_and_maybe_throw`].
    pub(super) fn remove_cycle_participants(&mut self, cycle: &Cycle) {
        for p in cycle.participant_keys() {
            let p: DependencyIndex = p.into();
            self.input_outputs.shift_remove(&(EdgeKind::Input, p));
        }
    }

    /// Copy the changed-at, durability, and dependencies from `cycle_query`.
    /// Used during cycle recovery, see [`Runtime::unblock_cycle_and_maybe_throw`].
    pub(crate) fn take_inputs_from(&mut self, cycle_query: &ActiveQuery) {
        self.changed_at = cycle_query.changed_at;
        self.durability = cycle_query.durability;
        self.input_outputs.clone_from(&cycle_query.input_outputs);
    }

    pub(super) fn disambiguate(&mut self, key: IdentityHash) -> Disambiguator {
        let disambiguator = self
            .disambiguator_map
            .entry(key)
            .or_insert(Disambiguator(0));
        let result = *disambiguator;
        disambiguator.0 += 1;
        result
    }
}
