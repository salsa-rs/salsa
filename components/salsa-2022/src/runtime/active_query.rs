use std::collections::BTreeSet;

use crate::{
    durability::Durability,
    hash::{FxHashSet, FxIndexMap, FxIndexSet},
    key::{DatabaseKeyIndex, DependencyIndex},
    tracked_struct::Disambiguator,
    Cycle, Revision, Runtime,
};

use super::local_state::{QueryInputs, QueryRevisions};

#[derive(Debug)]
pub(super) struct ActiveQuery {
    /// What query is executing
    pub(super) database_key_index: DatabaseKeyIndex,

    /// Minimum durability of inputs observed so far.
    pub(super) durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    pub(super) changed_at: Revision,

    /// Set of subqueries that were accessed thus far.
    pub(super) dependencies: FxIndexSet<DependencyIndex>,

    /// True if there was an untracked read.
    pub(super) untracked_read: bool,

    /// Stores the entire cycle, if one is found and this query is part of it.
    pub(super) cycle: Option<Cycle>,

    /// When new entities are created, their data is hashed, and the resulting
    /// hash is added to this map. If it is not present, then the disambiguator is 0.
    /// Otherwise it is 1 more than the current value (which is incremented).
    pub(super) disambiguator_map: FxIndexMap<u64, Disambiguator>,

    /// Tracks values written by this query. Could be...
    ///
    /// * tracked structs created
    /// * invocations of `specify`
    /// * accumulators pushed to
    ///
    /// We use a btree-set because we want to be able to
    /// extract the keys in sorted order.
    pub(super) outputs: BTreeSet<DatabaseKeyIndex>,
}

impl ActiveQuery {
    pub(super) fn new(database_key_index: DatabaseKeyIndex) -> Self {
        ActiveQuery {
            database_key_index,
            durability: Durability::MAX,
            changed_at: Revision::start(),
            dependencies: FxIndexSet::default(),
            untracked_read: false,
            cycle: None,
            disambiguator_map: Default::default(),
            outputs: Default::default(),
        }
    }

    pub(super) fn add_read(
        &mut self,
        input: DependencyIndex,
        durability: Durability,
        revision: Revision,
    ) {
        self.dependencies.insert(input);
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
    }

    pub(super) fn add_untracked_read(&mut self, changed_at: Revision) {
        self.untracked_read = true;
        self.durability = Durability::LOW;
        self.changed_at = changed_at;
    }

    pub(super) fn add_synthetic_read(&mut self, durability: Durability, revision: Revision) {
        self.untracked_read = true;
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
    }

    /// Adds a key to our list of outputs.
    pub(super) fn add_output(&mut self, key: DatabaseKeyIndex) {
        self.outputs.insert(key);
    }

    /// True if the given key was output by this query.
    pub(super) fn is_output(&self, key: DatabaseKeyIndex) -> bool {
        self.outputs.contains(&key)
    }

    pub(crate) fn revisions(&self, runtime: &Runtime) -> QueryRevisions {
        let inputs = QueryInputs {
            untracked: self.untracked_read,
            tracked: if self.dependencies.is_empty() {
                runtime.empty_dependencies()
            } else {
                self.dependencies.iter().copied().collect()
            },
        };

        QueryRevisions {
            changed_at: self.changed_at,
            inputs,
            durability: self.durability,
        }
    }

    /// Adds any dependencies from `other` into `self`.
    /// Used during cycle recovery, see [`Runtime::create_cycle_error`].
    pub(super) fn add_from(&mut self, other: &ActiveQuery) {
        self.changed_at = self.changed_at.max(other.changed_at);
        self.durability = self.durability.min(other.durability);
        self.untracked_read |= other.untracked_read;
        self.dependencies.extend(other.dependencies.iter().copied());
    }

    /// Removes the participants in `cycle` from my dependencies.
    /// Used during cycle recovery, see [`Runtime::create_cycle_error`].
    pub(super) fn remove_cycle_participants(&mut self, cycle: &Cycle) {
        for p in cycle.participant_keys() {
            let p: DependencyIndex = p.into();
            self.dependencies.remove(&p);
        }
    }

    /// Copy the changed-at, durability, and dependencies from `cycle_query`.
    /// Used during cycle recovery, see [`Runtime::create_cycle_error`].
    pub(crate) fn take_inputs_from(&mut self, cycle_query: &ActiveQuery) {
        self.changed_at = cycle_query.changed_at;
        self.durability = cycle_query.durability;
        self.dependencies = cycle_query.dependencies.clone();
    }

    pub(super) fn disambiguate(&mut self, hash: u64) -> Disambiguator {
        let disambiguator = self
            .disambiguator_map
            .entry(hash)
            .or_insert(Disambiguator(0));
        let result = *disambiguator;
        disambiguator.0 += 1;
        result
    }
}
