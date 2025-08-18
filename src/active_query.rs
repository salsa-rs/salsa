use std::{fmt, mem, ops};

#[cfg(feature = "accumulator")]
use crate::accumulator::{
    accumulated_map::{AccumulatedMap, AtomicInputAccumulatedValues, InputAccumulatedValues},
    Accumulator,
};
use crate::hash::FxIndexSet;
use crate::key::DatabaseKeyIndex;
use crate::runtime::Stamp;
use crate::sync::atomic::AtomicBool;
use crate::tracked_struct::{Disambiguator, DisambiguatorMap, IdentityHash, IdentityMap};
use crate::zalsa_local::{QueryEdge, QueryOrigin, QueryRevisions, QueryRevisionsExtra};
use crate::Revision;
use crate::{
    cycle::{CycleHeads, IterationCount},
    Id,
};
use crate::{durability::Durability, tracked_struct::Identity};

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
    tracked_struct_ids: IdentityMap,

    /// Stores the values accumulated to the given ingredient.
    /// The type of accumulated value is erased but known to the ingredient.
    #[cfg(feature = "accumulator")]
    accumulated: AccumulatedMap,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any accumulated values.
    #[cfg(feature = "accumulator")]
    accumulated_inputs: InputAccumulatedValues,

    /// Provisional cycle results that this query depends on.
    cycle_heads: CycleHeads,

    /// If this query is a cycle head, iteration count of that cycle.
    iteration_count: IterationCount,
}

impl ActiveQuery {
    pub(super) fn seed_iteration(
        &mut self,
        durability: Durability,
        changed_at: Revision,
        edges: &[QueryEdge],
        untracked_read: bool,
        active_tracked_ids: &[(Identity, Id)],
    ) {
        assert!(self.input_outputs.is_empty());

        self.input_outputs.extend(edges.iter().cloned());
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(changed_at);
        self.untracked_read |= untracked_read;

        // Mark all tracked structs from the previous iteration as active.
        self.tracked_struct_ids
            .mark_all_active(active_tracked_ids.iter().copied());
    }

    pub(super) fn add_read(
        &mut self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
        cycle_heads: &CycleHeads,
        #[cfg(feature = "accumulator")] has_accumulated: bool,
        #[cfg(feature = "accumulator")] accumulated_inputs: &AtomicInputAccumulatedValues,
    ) {
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(changed_at);
        self.input_outputs.insert(QueryEdge::input(input));
        self.cycle_heads.extend(cycle_heads);
        #[cfg(feature = "accumulator")]
        {
            self.accumulated_inputs = self.accumulated_inputs.or_else(|| match has_accumulated {
                true => InputAccumulatedValues::Any,
                false => accumulated_inputs.load(),
            });
        }
    }

    pub(super) fn add_read_simple(
        &mut self,
        input: DatabaseKeyIndex,
        durability: Durability,
        revision: Revision,
    ) {
        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
        self.input_outputs.insert(QueryEdge::input(input));
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

    #[cfg(feature = "accumulator")]
    pub(super) fn accumulate(&mut self, index: crate::IngredientIndex, value: impl Accumulator) {
        self.accumulated.accumulate(index, value);
    }

    /// Adds a key to our list of outputs.
    pub(super) fn add_output(&mut self, key: DatabaseKeyIndex) {
        self.input_outputs.insert(QueryEdge::output(key));
    }

    /// True if the given key was output by this query.
    pub(super) fn disambiguate(&mut self, key: IdentityHash) -> Disambiguator {
        self.disambiguator_map.disambiguate(key)
    }

    pub(super) fn stamp(&self) -> Stamp {
        Stamp {
            durability: self.durability,
            changed_at: self.changed_at,
        }
    }

    pub(super) fn iteration_count(&self) -> IterationCount {
        self.iteration_count
    }

    pub(crate) fn tracked_struct_ids(&self) -> &IdentityMap {
        &self.tracked_struct_ids
    }

    pub(crate) fn tracked_struct_ids_mut(&mut self) -> &mut IdentityMap {
        &mut self.tracked_struct_ids
    }
}

impl ActiveQuery {
    fn new(database_key_index: DatabaseKeyIndex, iteration_count: IterationCount) -> Self {
        ActiveQuery {
            database_key_index,
            durability: Durability::MAX,
            changed_at: Revision::start(),
            input_outputs: FxIndexSet::default(),
            untracked_read: false,
            disambiguator_map: Default::default(),
            tracked_struct_ids: Default::default(),
            cycle_heads: Default::default(),
            iteration_count,
            #[cfg(feature = "accumulator")]
            accumulated: Default::default(),
            #[cfg(feature = "accumulator")]
            accumulated_inputs: Default::default(),
        }
    }

    fn top_into_revisions(&mut self) -> CompletedQuery {
        let &mut Self {
            database_key_index: _,
            durability,
            changed_at,
            ref mut input_outputs,
            untracked_read,
            ref mut disambiguator_map,
            ref mut tracked_struct_ids,
            ref mut cycle_heads,
            iteration_count,
            #[cfg(feature = "accumulator")]
            ref mut accumulated,
            #[cfg(feature = "accumulator")]
            accumulated_inputs,
        } = self;

        let origin = if untracked_read {
            QueryOrigin::derived_untracked(input_outputs.drain(..).collect())
        } else {
            QueryOrigin::derived(input_outputs.drain(..).collect())
        };
        disambiguator_map.clear();

        #[cfg(feature = "accumulator")]
        let accumulated_inputs = AtomicInputAccumulatedValues::new(accumulated_inputs);
        let verified_final = cycle_heads.is_empty();
        let (active_tracked_structs, stale_tracked_structs) = tracked_struct_ids.drain();

        let extra = QueryRevisionsExtra::new(
            #[cfg(feature = "accumulator")]
            mem::take(accumulated),
            active_tracked_structs,
            mem::take(cycle_heads),
            iteration_count,
        );

        let revisions = QueryRevisions {
            changed_at,
            durability,
            origin,
            #[cfg(feature = "accumulator")]
            accumulated_inputs,
            verified_final: AtomicBool::new(verified_final),
            extra,
        };

        CompletedQuery {
            revisions,
            stale_tracked_structs,
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
            cycle_heads,
            iteration_count,
            #[cfg(feature = "accumulator")]
            accumulated,
            #[cfg(feature = "accumulator")]
                accumulated_inputs: _,
        } = self;
        input_outputs.clear();
        disambiguator_map.clear();
        tracked_struct_ids.clear();
        *cycle_heads = Default::default();
        *iteration_count = IterationCount::initial();
        #[cfg(feature = "accumulator")]
        accumulated.clear();
    }

    fn reset_for(
        &mut self,
        new_database_key_index: DatabaseKeyIndex,
        new_iteration_count: IterationCount,
    ) {
        let Self {
            database_key_index,
            durability,
            changed_at,
            input_outputs,
            untracked_read,
            disambiguator_map,
            tracked_struct_ids,
            cycle_heads,
            iteration_count,
            #[cfg(feature = "accumulator")]
            accumulated,
            #[cfg(feature = "accumulator")]
            accumulated_inputs,
        } = self;
        *database_key_index = new_database_key_index;
        *durability = Durability::MAX;
        *changed_at = Revision::start();
        *untracked_read = false;
        *iteration_count = new_iteration_count;
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
        #[cfg(feature = "accumulator")]
        {
            *accumulated_inputs = Default::default();
            debug_assert!(
                accumulated.is_empty(),
                "`ActiveQuery::clear` or `ActiveQuery::into_revisions` should've been called"
            );
        }
    }
}

#[derive(Default)]
pub(crate) struct QueryStack {
    stack: Vec<ActiveQuery>,
    len: usize,
}

impl std::fmt::Debug for QueryStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            f.debug_list()
                .entries(self.stack.iter().map(|q| q.database_key_index))
                .finish()
        } else {
            f.debug_struct("QueryStack")
                .field("stack", &self.stack)
                .field("len", &self.len)
                .finish()
        }
    }
}

impl ops::Deref for QueryStack {
    type Target = [ActiveQuery];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.stack[..self.len]
    }
}

impl ops::DerefMut for QueryStack {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stack[..self.len]
    }
}

impl QueryStack {
    pub(crate) fn push_new_query(
        &mut self,
        database_key_index: DatabaseKeyIndex,
        iteration_count: IterationCount,
    ) {
        if self.len < self.stack.len() {
            self.stack[self.len].reset_for(database_key_index, iteration_count);
        } else {
            self.stack
                .push(ActiveQuery::new(database_key_index, iteration_count));
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
    ) -> CompletedQuery {
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

/// The state of a completed query.
pub(crate) struct CompletedQuery {
    /// Inputs and outputs accumulated during query execution.
    pub(crate) revisions: QueryRevisions,

    /// The keys of any tracked structs that were created in a previous execution of the
    /// query but not the current one, and should be marked as stale.
    pub(crate) stale_tracked_structs: Vec<(Identity, Id)>,
}

struct CapturedQuery {
    database_key_index: DatabaseKeyIndex,
    durability: Durability,
    changed_at: Revision,
    cycle_heads: CycleHeads,
    iteration_count: IterationCount,
}

impl fmt::Debug for CapturedQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("CapturedQuery");
        debug_struct
            .field("database_key_index", &self.database_key_index)
            .field("durability", &self.durability)
            .field("changed_at", &self.changed_at);
        if !self.cycle_heads.is_empty() {
            debug_struct
                .field("cycle_heads", &self.cycle_heads)
                .field("iteration_count", &self.iteration_count);
        }
        debug_struct.finish()
    }
}

pub struct Backtrace(Box<[CapturedQuery]>);

impl Backtrace {
    pub fn capture() -> Option<Self> {
        crate::with_attached_database(|db| {
            db.zalsa_local().try_with_query_stack(|stack| {
                Backtrace(
                    stack
                        .iter()
                        .rev()
                        .map(|query| CapturedQuery {
                            database_key_index: query.database_key_index,
                            durability: query.durability,
                            changed_at: query.changed_at,
                            cycle_heads: query.cycle_heads.clone(),
                            iteration_count: query.iteration_count,
                        })
                        .collect(),
                )
            })
        })?
    }
}

impl fmt::Debug for Backtrace {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Backtrace ")?;

        let mut dbg = fmt.debug_list();

        for frame in &self.0 {
            dbg.entry(&frame);
        }

        dbg.finish()
    }
}

impl fmt::Display for Backtrace {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(fmt, "query stacktrace:")?;
        let full = fmt.alternate();
        let indent = "             ";
        for (
            idx,
            &CapturedQuery {
                database_key_index,
                durability,
                changed_at,
                ref cycle_heads,
                iteration_count,
            },
        ) in self.0.iter().enumerate()
        {
            write!(fmt, "{idx:>4}: {database_key_index:?}")?;
            if full {
                write!(fmt, " -> ({changed_at:?}, {durability:#?}")?;
                if !cycle_heads.is_empty() || !iteration_count.is_initial() {
                    write!(fmt, ", iteration = {iteration_count:?}")?;
                }
                write!(fmt, ")")?;
            }
            writeln!(fmt)?;
            crate::attach::with_attached_database(|db| {
                let ingredient = db
                    .zalsa()
                    .lookup_ingredient(database_key_index.ingredient_index());
                let loc = ingredient.location();
                writeln!(fmt, "{indent}at {}:{}", loc.file, loc.line)?;
                if !cycle_heads.is_empty() {
                    write!(fmt, "{indent}cycle heads: ")?;
                    for (idx, head) in cycle_heads.iter().enumerate() {
                        if idx != 0 {
                            write!(fmt, ", ")?;
                        }
                        write!(
                            fmt,
                            "{:?} -> {:?}",
                            head.database_key_index, head.iteration_count
                        )?;
                    }
                    writeln!(fmt)?;
                }
                Ok(())
            })
            .transpose()?;
        }
        Ok(())
    }
}
