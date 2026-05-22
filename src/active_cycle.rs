use std::num::NonZeroUsize;

use rustc_hash::FxHashMap;

use crate::cycle::{CycleHeads, IterationCount};
use crate::hash::FxIndexSet;
use crate::key::DatabaseKeyIndex;
use crate::sync::Mutex;
use crate::zalsa_local::{QueryEdge, QueryEdgeKind};

const INDEX_BITS: u32 = usize::BITS / 2;
const INDEX_MASK: usize = (1usize << INDEX_BITS) - 1;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ActiveCycleKey(NonZeroUsize);

impl ActiveCycleKey {
    fn new(index: usize, generation: usize) -> Self {
        debug_assert!(index <= INDEX_MASK);
        debug_assert!(generation != 0);
        debug_assert!(generation <= INDEX_MASK);

        // Store a one-based index so that `0` remains available as the `Option` niche.
        let packed = (generation << INDEX_BITS) | (index + 1);
        Self(NonZeroUsize::new(packed).expect("packed active cycle key must be non-zero"))
    }

    fn index(self) -> usize {
        (self.0.get() & INDEX_MASK) - 1
    }

    fn generation(self) -> usize {
        self.0.get() >> INDEX_BITS
    }

    pub(crate) fn from_raw(raw: usize) -> Option<Self> {
        NonZeroUsize::new(raw).map(Self)
    }

    pub(crate) fn raw(self) -> usize {
        self.0.get()
    }
}

#[derive(Debug)]
pub(crate) struct ActiveCycle {
    converged: bool,
    pub(crate) iteration: IterationCount,
    current_heads: CycleHeads,
    // External inputs observed by provisional memos across every cycle iteration.
    input_dependencies: FxIndexSet<DatabaseKeyIndex>,
}

#[derive(Copy, Clone, Debug)]
struct ActiveCycleMemo {
    active_cycle: ActiveCycleKey,
    is_head: bool,
    last_iteration: Option<IterationCount>,
}

impl ActiveCycleMemo {
    fn head(active_cycle: ActiveCycleKey, iteration: IterationCount) -> Self {
        Self {
            active_cycle,
            is_head: true,
            last_iteration: Some(iteration),
        }
    }

    fn participant(active_cycle: ActiveCycleKey, iteration: IterationCount) -> Self {
        Self {
            active_cycle,
            is_head: false,
            last_iteration: Some(iteration),
        }
    }

    fn is_current(self, iteration: IterationCount) -> bool {
        self.last_iteration == Some(iteration)
    }
}

impl ActiveCycle {
    fn new(head: DatabaseKeyIndex, iteration: IterationCount) -> Self {
        let mut current_heads = CycleHeads::default();
        current_heads.insert(head);
        Self {
            converged: true,
            iteration,
            current_heads,
            input_dependencies: FxIndexSet::default(),
        }
    }

    fn remove_current_head(&mut self, memo: DatabaseKeyIndex) {
        self.current_heads.remove(memo);
    }

    fn current_heads(&self) -> CycleHeads {
        self.current_heads.clone()
    }

    fn set_converged(&mut self, converged: bool) {
        self.converged &= converged;
    }

    fn start_next_iteration(&mut self, iteration: IterationCount) {
        self.iteration = iteration;
        self.converged = true;
        self.current_heads = CycleHeads::default();
    }

    fn merge_from(&mut self, other: ActiveCycle) {
        let iteration = self.iteration.max(other.iteration);
        self.converged &= other.converged;
        self.iteration = iteration;
        self.current_heads.extend(&other.current_heads);
        self.input_dependencies.extend(other.input_dependencies);
    }
}

#[derive(Debug)]
struct ActiveCycleSlot {
    generation: usize,
    state: Option<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct ActiveCycles {
    slots: Vec<ActiveCycleSlot>,
    free_slots: Vec<usize>,
    states: Vec<Option<ActiveCycle>>,
    free_states: Vec<usize>,
    memo_cycles: FxHashMap<DatabaseKeyIndex, ActiveCycleMemo>,
}

impl ActiveCycles {
    pub(crate) fn insert(
        &mut self,
        head: DatabaseKeyIndex,
        iteration: IterationCount,
    ) -> ActiveCycleKey {
        let state = self.insert_state(ActiveCycle::new(head, iteration));

        let key = if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.slots[index];
            slot.generation = slot
                .generation
                .checked_add(1)
                .expect("active cycle generation overflow");
            if slot.generation > INDEX_MASK {
                panic!("active cycle generation overflow");
            }
            slot.state = Some(state);
            ActiveCycleKey::new(index, slot.generation)
        } else {
            let index = self.slots.len();
            assert!(index < INDEX_MASK, "too many active cycles");
            let generation = 1;
            self.slots.push(ActiveCycleSlot {
                generation,
                state: Some(state),
            });
            ActiveCycleKey::new(index, generation)
        };

        self.memo_cycles
            .insert(head, ActiveCycleMemo::head(key, iteration));
        key
    }

    fn insert_state(&mut self, cycle: ActiveCycle) -> usize {
        if let Some(index) = self.free_states.pop() {
            self.states[index] = Some(cycle);
            index
        } else {
            let index = self.states.len();
            self.states.push(Some(cycle));
            index
        }
    }

    pub(crate) fn remove(&mut self, key: ActiveCycleKey) -> Option<ActiveCycle> {
        let state = self.state_for(key)?;

        let stale_memos: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, cycle)| {
                (self.state_for(cycle.active_cycle) == Some(state)).then_some(*memo)
            })
            .collect();
        for memo in stale_memos {
            self.memo_cycles.remove(&memo);
        }

        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == Some(state) {
                slot.state = None;
                self.free_slots.push(index);
            }
        }

        self.take_state(state)
    }

    fn take_state(&mut self, state: usize) -> Option<ActiveCycle> {
        let cycle = self.states.get_mut(state)?.take()?;
        self.free_states.push(state);
        Some(cycle)
    }

    pub(crate) fn get(&self, key: ActiveCycleKey) -> Option<&ActiveCycle> {
        let state = self.state_for(key)?;
        self.states.get(state)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, key: ActiveCycleKey) -> Option<&mut ActiveCycle> {
        let state = self.state_for(key)?;
        self.states.get_mut(state)?.as_mut()
    }

    fn state_for(&self, key: ActiveCycleKey) -> Option<usize> {
        let slot = self.slots.get(key.index())?;
        if slot.generation == key.generation() {
            slot.state
        } else {
            None
        }
    }

    pub(crate) fn merge(&mut self, into: ActiveCycleKey, from: ActiveCycleKey) -> Option<()> {
        let into_state = self.state_for(into)?;
        let from_state = self.state_for(from)?;

        if into_state == from_state {
            return Some(());
        }

        let into_iteration = self.get(into)?.iteration;
        let from_iteration = self.get(from)?.iteration;
        let iteration = into_iteration.max(from_iteration);
        let updated_memos: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, cycle)| {
                (self.state_for(cycle.active_cycle) == Some(into_state)
                    && cycle.is_current(into_iteration))
                .then_some(*memo)
            })
            .collect();
        let remapped_memos: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, cycle)| {
                (self.state_for(cycle.active_cycle) == Some(from_state)).then_some(*memo)
            })
            .collect();

        let from_cycle = self.take_state(from_state)?;
        self.get_mut(into)?.merge_from(from_cycle);

        for slot in &mut self.slots {
            if slot.state == Some(from_state) {
                slot.state = Some(into_state);
            }
        }
        for memo in updated_memos {
            if let Some(cycle) = self.memo_cycles.get_mut(&memo) {
                cycle.last_iteration = Some(iteration);
            }
        }
        for memo in remapped_memos {
            if let Some(cycle) = self.memo_cycles.get_mut(&memo) {
                cycle.active_cycle = into;
                if cycle.is_current(from_iteration) {
                    cycle.last_iteration = Some(iteration);
                }
            }
        }

        let participants: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, cycle)| {
                (self.state_for(cycle.active_cycle) == Some(into_state)).then_some(*memo)
            })
            .collect();
        if let Some(cycle) = self.get_mut(into) {
            for participant in participants {
                cycle.input_dependencies.shift_remove(&participant);
            }
        }

        Some(())
    }

    fn add_participant(
        &mut self,
        active_cycle: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> Option<()> {
        let state = self.state_for(active_cycle)?;
        let iteration = self.get(active_cycle)?.iteration;
        if let Some(previous) = self.memo_cycles.get(&memo).copied() {
            if self.state_for(previous.active_cycle) == Some(state) {
                if previous.is_current(iteration) {
                    return Some(());
                }

                let cycle = self.memo_cycles.get_mut(&memo)?;
                cycle.active_cycle = active_cycle;
                cycle.last_iteration = Some(iteration);
                if previous.is_head {
                    self.get_mut(active_cycle)?.current_heads.insert(memo);
                }
                self.get_mut(active_cycle)?
                    .input_dependencies
                    .shift_remove(&memo);
                return Some(());
            }

            self.get_mut(previous.active_cycle)?
                .remove_current_head(memo);
        }

        self.memo_cycles
            .insert(memo, ActiveCycleMemo::participant(active_cycle, iteration));
        self.get_mut(active_cycle)?
            .input_dependencies
            .shift_remove(&memo);
        Some(())
    }

    fn add_input_edges(&mut self, active_cycle: ActiveCycleKey, edges: &[QueryEdge]) -> Option<()> {
        let state = self.state_for(active_cycle)?;
        for edge in edges {
            let QueryEdgeKind::Input(input) = edge.kind() else {
                continue;
            };

            let internal = self
                .memo_cycles
                .get(&input)
                .is_some_and(|cycle| self.state_for(cycle.active_cycle) == Some(state));
            if !internal {
                self.get_mut(active_cycle)?.input_dependencies.insert(input);
            }
        }
        Some(())
    }

    fn flattened_inputs(
        &mut self,
        active_cycle: ActiveCycleKey,
        edges: &[QueryEdge],
    ) -> Option<Vec<DatabaseKeyIndex>> {
        self.add_input_edges(active_cycle, edges)?;

        Some(
            self.get(active_cycle)?
                .input_dependencies
                .iter()
                .copied()
                .collect(),
        )
    }

    fn add_head(&mut self, active_cycle: ActiveCycleKey, head: DatabaseKeyIndex) -> Option<()> {
        let state = self.state_for(active_cycle)?;
        let iteration = self.get(active_cycle)?.iteration;
        if let Some(previous) = self.memo_cycles.get(&head).copied() {
            if self.state_for(previous.active_cycle) == Some(state) {
                let cycle = self.memo_cycles.get_mut(&head)?;
                cycle.active_cycle = active_cycle;
                cycle.is_head = true;
                if previous.is_current(iteration) {
                    self.get_mut(active_cycle)?.current_heads.insert(head);
                }
                self.get_mut(active_cycle)?
                    .input_dependencies
                    .shift_remove(&head);
                return Some(());
            }

            self.get_mut(previous.active_cycle)?
                .remove_current_head(head);
        }

        self.memo_cycles.insert(
            head,
            ActiveCycleMemo {
                active_cycle,
                is_head: true,
                last_iteration: None,
            },
        );
        self.get_mut(active_cycle)?
            .input_dependencies
            .shift_remove(&head);
        Some(())
    }

    fn contains_current_iteration(
        &self,
        active_cycle: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> bool {
        let Some(state) = self.state_for(active_cycle) else {
            return false;
        };
        let Some(iteration) = self.get(active_cycle).map(|cycle| cycle.iteration) else {
            return false;
        };
        self.memo_cycles.get(&memo).is_some_and(|cycle| {
            self.state_for(cycle.active_cycle) == Some(state) && cycle.is_current(iteration)
        })
    }

    fn current_memo_keys(&self, active_cycle: ActiveCycleKey) -> Option<Vec<DatabaseKeyIndex>> {
        let state = self.state_for(active_cycle)?;
        let iteration = self.get(active_cycle)?.iteration;
        let mut keys: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, cycle)| {
                (self.state_for(cycle.active_cycle) == Some(state) && cycle.is_current(iteration))
                    .then_some(*memo)
            })
            .collect();
        keys.sort_by_key(|key| (key.ingredient_index(), key.key_index()));
        Some(keys)
    }

    fn heads_are_covered_by(
        &self,
        active_cycle: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<bool> {
        let state = self.state_for(active_cycle)?;
        Some(self.memo_cycles.iter().all(|(memo, cycle)| {
            self.state_for(cycle.active_cycle) != Some(state)
                || !cycle.is_head
                || cycle_heads.contains(memo)
        }))
    }

    fn take_memo_keys(
        &mut self,
        active_cycle: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<Vec<DatabaseKeyIndex>> {
        self.state_for(active_cycle)?;
        let keys: Vec<_> = cycle_heads
            .iter()
            .map(|head| head.database_key_index)
            .collect();
        for key in &keys {
            self.remove_memo(*key);
        }
        Some(keys)
    }

    fn remove_memo(&mut self, memo: DatabaseKeyIndex) -> Option<ActiveCycleMemo> {
        let cycle = self.memo_cycles.remove(&memo)?;
        if let Some(active_cycle) = self.get_mut(cycle.active_cycle) {
            active_cycle.remove_current_head(memo);
        }
        Some(cycle)
    }

    fn add_heads(
        &mut self,
        active_cycle: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<IterationCount> {
        for head in cycle_heads {
            if let Some(head_cycle) = self.memo_cycles.get(&head.database_key_index).copied() {
                if head_cycle.active_cycle != active_cycle {
                    self.merge(active_cycle, head_cycle.active_cycle);
                }
            }

            self.add_head(active_cycle, head.database_key_index)?;
        }

        self.get(active_cycle).map(|cycle| cycle.iteration)
    }

    fn reuse_participant(
        &mut self,
        current: Option<ActiveCycleKey>,
        memo_cycle: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> Option<ActiveCycleKey> {
        let memo_state = self.state_for(memo_cycle)?;
        let memo_key = self.memo_cycles.get(&memo)?;
        if self.state_for(memo_key.active_cycle) != Some(memo_state) {
            return None;
        }

        let active_cycle = if let Some(current) = current {
            self.merge(current, memo_cycle)?;
            current
        } else if self.contains_current_iteration(memo_cycle, memo) {
            memo_cycle
        } else {
            let iteration = self.get(memo_cycle)?.iteration;
            self.remove_memo(memo)?;
            self.insert(memo, iteration)
        };

        self.add_head(active_cycle, memo)?;
        self.add_participant(active_cycle, memo)?;

        Some(active_cycle)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ActiveCycleTable(Mutex<ActiveCycles>);

impl ActiveCycleTable {
    pub(crate) fn insert(
        &self,
        head: DatabaseKeyIndex,
        iteration: IterationCount,
    ) -> ActiveCycleKey {
        self.0.lock().insert(head, iteration)
    }

    pub(crate) fn remove(&self, key: ActiveCycleKey) -> Option<ActiveCycle> {
        self.0.lock().remove(key)
    }

    pub(crate) fn converged(&self, key: ActiveCycleKey) -> Option<bool> {
        self.0.lock().get(key).map(|cycle| cycle.converged)
    }

    pub(crate) fn add_participant(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> Option<()> {
        self.0.lock().add_participant(key, memo)
    }

    pub(crate) fn add_input_edges(&self, key: ActiveCycleKey, edges: &[QueryEdge]) -> Option<()> {
        self.0.lock().add_input_edges(key, edges)
    }

    pub(crate) fn flattened_inputs(
        &self,
        key: ActiveCycleKey,
        edges: &[QueryEdge],
    ) -> Option<Vec<DatabaseKeyIndex>> {
        self.0.lock().flattened_inputs(key, edges)
    }

    pub(crate) fn contains_current_iteration(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> bool {
        self.0.lock().contains_current_iteration(key, memo)
    }

    pub(crate) fn key_for(&self, memo: DatabaseKeyIndex) -> Option<ActiveCycleKey> {
        self.0
            .lock()
            .memo_cycles
            .get(&memo)
            .map(|cycle| cycle.active_cycle)
    }

    pub(crate) fn current_memo_keys(&self, key: ActiveCycleKey) -> Option<Vec<DatabaseKeyIndex>> {
        self.0.lock().current_memo_keys(key)
    }

    pub(crate) fn heads_are_covered_by(
        &self,
        key: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<bool> {
        self.0.lock().heads_are_covered_by(key, cycle_heads)
    }

    pub(crate) fn take_memo_keys(
        &self,
        key: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<Vec<DatabaseKeyIndex>> {
        self.0.lock().take_memo_keys(key, cycle_heads)
    }

    pub(crate) fn current_heads(&self, key: ActiveCycleKey) -> Option<CycleHeads> {
        self.0.lock().get(key).map(ActiveCycle::current_heads)
    }

    pub(crate) fn current_heads_for_memo(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> Option<CycleHeads> {
        let cycles = self.0.lock();
        cycles.contains_current_iteration(key, memo).then_some(())?;
        let cycle = cycles.get(key)?;
        Some(cycle.current_heads())
    }

    pub(crate) fn iteration(&self, key: ActiveCycleKey) -> Option<IterationCount> {
        self.0.lock().get(key).map(|cycle| cycle.iteration)
    }

    pub(crate) fn add_head(&self, key: ActiveCycleKey, head: DatabaseKeyIndex) -> Option<()> {
        self.0.lock().add_head(key, head)
    }

    pub(crate) fn add_heads(
        &self,
        active_cycle: Option<ActiveCycleKey>,
        cycle_heads: &CycleHeads,
    ) -> (Option<ActiveCycleKey>, Option<IterationCount>) {
        let mut cycles = self.0.lock();
        let active_cycle = active_cycle.or_else(|| {
            cycle_heads.iter().find_map(|head| {
                cycles
                    .memo_cycles
                    .get(&head.database_key_index)
                    .map(|cycle| cycle.active_cycle)
            })
        });
        let iteration =
            active_cycle.and_then(|active_cycle| cycles.add_heads(active_cycle, cycle_heads));
        (active_cycle, iteration)
    }

    pub(crate) fn reuse_participant(
        &self,
        current: Option<ActiveCycleKey>,
        memo_cycle: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> Option<ActiveCycleKey> {
        self.0.lock().reuse_participant(current, memo_cycle, memo)
    }

    pub(crate) fn start_next_iteration(
        &self,
        key: ActiveCycleKey,
        iteration: IterationCount,
    ) -> Option<()> {
        self.with_mut(key, |cycle| {
            cycle.start_next_iteration(iteration);
        })
    }

    pub(crate) fn set_converged(&self, key: ActiveCycleKey, converged: bool) -> Option<()> {
        self.with_mut(key, |cycle| {
            cycle.set_converged(converged);
        })
    }

    fn with_mut<R>(&self, key: ActiveCycleKey, f: impl FnOnce(&mut ActiveCycle) -> R) -> Option<R> {
        let mut cycles = self.0.lock();
        cycles.get_mut(key).map(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Id;
    use crate::zalsa::IngredientIndex;

    fn database_key(index: u32) -> DatabaseKeyIndex {
        // SAFETY: The test only needs distinct IDs to construct database keys.
        DatabaseKeyIndex::new(IngredientIndex::new(0), unsafe { Id::from_index(index) })
    }

    #[test]
    fn merge_remaps_all_cycle_keys_for_state() {
        let mut cycles = ActiveCycles::default();
        let head_a = database_key(0);
        let head_b = database_key(1);
        let participant = database_key(2);

        let cycle_a = cycles.insert(head_a, IterationCount::initial());
        let cycle_b = cycles.insert(head_b, IterationCount::initial());
        cycles.add_participant(cycle_b, participant).unwrap();

        cycles.merge(cycle_a, cycle_b).unwrap();

        assert_eq!(cycles.state_for(cycle_a), cycles.state_for(cycle_b));
        assert_eq!(
            cycles
                .memo_cycles
                .get(&head_b)
                .map(|cycle| cycle.active_cycle),
            Some(cycle_a)
        );
        assert_eq!(
            cycles
                .memo_cycles
                .get(&participant)
                .map(|cycle| cycle.active_cycle),
            Some(cycle_a)
        );

        cycles.remove(cycle_b).unwrap();

        assert!(cycles.get(cycle_a).is_none());
        assert!(cycles.get(cycle_b).is_none());
        assert!(!cycles.memo_cycles.contains_key(&head_a));
        assert!(!cycles.memo_cycles.contains_key(&head_b));
        assert!(!cycles.memo_cycles.contains_key(&participant));
    }

    #[test]
    fn merge_keeps_participants_current_at_the_merged_iteration() {
        let mut cycles = ActiveCycles::default();
        let cycle_a = cycles.insert(database_key(0), IterationCount::initial());
        cycles.add_participant(cycle_a, database_key(1)).unwrap();
        let cycle_b = cycles.insert(database_key(2), IterationCount::initial());
        let next_iteration = IterationCount::initial().increment().unwrap();
        cycles
            .get_mut(cycle_b)
            .unwrap()
            .start_next_iteration(next_iteration);
        cycles.add_participant(cycle_b, database_key(3)).unwrap();

        cycles.merge(cycle_a, cycle_b).unwrap();

        assert_eq!(cycles.get(cycle_a).unwrap().iteration, next_iteration);
        assert_eq!(
            cycles.current_memo_keys(cycle_a).unwrap(),
            vec![database_key(0), database_key(1), database_key(3)]
        );
    }
}
