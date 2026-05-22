use std::num::NonZeroUsize;

use crate::cycle::{CycleHeads, IterationCount};
use crate::hash::FxIndexMap;
use crate::key::DatabaseKeyIndex;
use crate::sync::Mutex;
use rustc_hash::FxHashMap;

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
}

#[derive(Debug)]
pub(crate) struct ActiveCycle {
    converged: bool,
    pub(crate) iteration: IterationCount,
    participants: FxIndexMap<DatabaseKeyIndex, ActiveCycleParticipant>,
}

#[derive(Copy, Clone, Debug)]
struct ActiveCycleParticipant {
    is_head: bool,
    last_iteration: Option<IterationCount>,
}

impl ActiveCycleParticipant {
    fn head(iteration: IterationCount) -> Self {
        Self {
            is_head: true,
            last_iteration: Some(iteration),
        }
    }

    fn participant(iteration: IterationCount) -> Self {
        Self {
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
        let mut participants = FxIndexMap::default();
        participants.insert(head, ActiveCycleParticipant::head(iteration));
        Self {
            converged: true,
            iteration,
            participants,
        }
    }

    fn add_participant(&mut self, memo: DatabaseKeyIndex) {
        self.participants
            .entry(memo)
            .and_modify(|participant| participant.last_iteration = Some(self.iteration))
            .or_insert_with(|| ActiveCycleParticipant::participant(self.iteration));
    }

    fn add_head(&mut self, head: DatabaseKeyIndex) {
        self.participants
            .entry(head)
            .and_modify(|participant| participant.is_head = true)
            .or_insert(ActiveCycleParticipant {
                is_head: true,
                last_iteration: None,
            });
    }

    fn contains_current_iteration(&self, memo: DatabaseKeyIndex) -> bool {
        self.participants
            .get(&memo)
            .is_some_and(|participant| participant.is_current(self.iteration))
    }

    fn current_memo_keys(&self) -> Vec<DatabaseKeyIndex> {
        let mut keys: Vec<_> = self
            .participants
            .iter()
            .filter_map(|(key, participant)| participant.is_current(self.iteration).then_some(*key))
            .collect();
        keys.sort_by_key(|key| (key.ingredient_index(), key.key_index()));
        keys
    }

    fn heads_are_covered_by(&self, cycle_heads: &CycleHeads) -> bool {
        self.participants
            .iter()
            .filter_map(|(key, participant)| participant.is_head.then_some(key))
            .all(|head| cycle_heads.contains(head))
    }

    fn take_memo_keys(&mut self, cycle_heads: &CycleHeads) -> Vec<DatabaseKeyIndex> {
        let keys: Vec<_> = cycle_heads
            .iter()
            .map(|head| head.database_key_index)
            .collect();
        for key in &keys {
            self.participants.swap_remove(key);
        }
        keys
    }

    fn current_heads(&self) -> CycleHeads {
        let mut cycle_heads = CycleHeads::default();
        for (key, participant) in &self.participants {
            if participant.is_head && participant.is_current(self.iteration) {
                cycle_heads.insert(*key);
            }
        }
        cycle_heads
    }

    fn set_converged(&mut self, converged: bool) {
        self.converged &= converged;
    }

    fn start_next_iteration(&mut self, iteration: IterationCount) {
        self.iteration = iteration;
        self.converged = true;
    }

    fn merge_from(&mut self, other: ActiveCycle) {
        let previous_iteration = self.iteration;
        let other_iteration = other.iteration;
        let iteration = previous_iteration.max(other_iteration);

        if iteration != previous_iteration {
            for participant in self.participants.values_mut() {
                if participant.is_current(previous_iteration) {
                    participant.last_iteration = Some(iteration);
                }
            }
        }

        self.converged &= other.converged;
        self.iteration = iteration;

        for (key, mut participant) in other.participants {
            if participant.is_current(other_iteration) {
                participant.last_iteration = Some(iteration);
            }

            self.participants
                .entry(key)
                .and_modify(|current| {
                    current.is_head |= participant.is_head;
                    current.last_iteration = current.last_iteration.max(participant.last_iteration);
                })
                .or_insert(participant);
        }
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
    memo_cycles: FxHashMap<DatabaseKeyIndex, ActiveCycleKey>,
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

        self.memo_cycles.insert(head, key);
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
            .filter_map(|(memo, key)| (self.state_for(*key) == Some(state)).then_some(*memo))
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

    fn cycle_for_memo(&self, key: ActiveCycleKey, memo: DatabaseKeyIndex) -> Option<&ActiveCycle> {
        let state = self.state_for(key)?;
        let memo_key = self.memo_cycles.get(&memo)?;
        if self.state_for(*memo_key) != Some(state) {
            return None;
        }
        self.states.get(state)?.as_ref()
    }

    pub(crate) fn merge(&mut self, into: ActiveCycleKey, from: ActiveCycleKey) -> Option<()> {
        let into_state = self.state_for(into)?;
        let from_state = self.state_for(from)?;

        if into_state == from_state {
            return Some(());
        }

        let remapped_memos: Vec<_> = self
            .memo_cycles
            .iter()
            .filter_map(|(memo, key)| (self.state_for(*key) == Some(from_state)).then_some(*memo))
            .collect();

        let from_cycle = self.take_state(from_state)?;
        self.get_mut(into)?.merge_from(from_cycle);

        for slot in &mut self.slots {
            if slot.state == Some(from_state) {
                slot.state = Some(into_state);
            }
        }
        for memo in remapped_memos {
            self.memo_cycles.insert(memo, into);
        }

        Some(())
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
        let mut cycles = self.0.lock();
        let cycle = cycles.get_mut(key)?;
        cycle.add_participant(memo);
        cycles.memo_cycles.insert(memo, key);
        Some(())
    }

    pub(crate) fn contains_current_iteration(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
    ) -> bool {
        let cycles = self.0.lock();
        cycles
            .cycle_for_memo(key, memo)
            .is_some_and(|cycle| cycle.contains_current_iteration(memo))
    }

    pub(crate) fn contains_participant(&self, key: ActiveCycleKey, memo: DatabaseKeyIndex) -> bool {
        let cycles = self.0.lock();
        let Some(state) = cycles.state_for(key) else {
            return false;
        };
        let Some(memo_key) = cycles.memo_cycles.get(&memo) else {
            return false;
        };
        cycles.state_for(*memo_key) == Some(state)
    }

    pub(crate) fn key_for(&self, memo: DatabaseKeyIndex) -> Option<ActiveCycleKey> {
        self.0.lock().memo_cycles.get(&memo).copied()
    }

    pub(crate) fn current_memo_keys(&self, key: ActiveCycleKey) -> Option<Vec<DatabaseKeyIndex>> {
        self.0.lock().get(key).map(ActiveCycle::current_memo_keys)
    }

    pub(crate) fn heads_are_covered_by(
        &self,
        key: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<bool> {
        self.0
            .lock()
            .get(key)
            .map(|active_cycle| active_cycle.heads_are_covered_by(cycle_heads))
    }

    pub(crate) fn take_memo_keys(
        &self,
        key: ActiveCycleKey,
        cycle_heads: &CycleHeads,
    ) -> Option<Vec<DatabaseKeyIndex>> {
        let mut cycles = self.0.lock();
        let state = cycles.state_for(key)?;
        let keys = cycles
            .states
            .get_mut(state)?
            .as_mut()?
            .take_memo_keys(cycle_heads);
        for key in &keys {
            cycles.memo_cycles.remove(key);
        }
        Some(keys)
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
        let cycle = cycles.cycle_for_memo(key, memo)?;
        cycle.contains_current_iteration(memo).then_some(())?;
        Some(cycle.current_heads())
    }

    pub(crate) fn iteration(&self, key: ActiveCycleKey) -> Option<IterationCount> {
        self.0.lock().get(key).map(|cycle| cycle.iteration)
    }

    pub(crate) fn add_head(&self, key: ActiveCycleKey, head: DatabaseKeyIndex) -> Option<()> {
        self.with_mut(key, |cycle| {
            cycle.add_head(head);
        })
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

    pub(crate) fn merge(&self, into: ActiveCycleKey, from: ActiveCycleKey) -> Option<()> {
        self.0.lock().merge(into, from)
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
        cycles
            .get_mut(cycle_b)
            .unwrap()
            .add_participant(participant);
        cycles.memo_cycles.insert(participant, cycle_b);

        cycles.merge(cycle_a, cycle_b).unwrap();

        assert_eq!(cycles.state_for(cycle_a), cycles.state_for(cycle_b));
        assert_eq!(cycles.memo_cycles.get(&head_b), Some(&cycle_a));
        assert_eq!(cycles.memo_cycles.get(&participant), Some(&cycle_a));

        cycles.remove(cycle_b).unwrap();

        assert!(cycles.get(cycle_a).is_none());
        assert!(cycles.get(cycle_b).is_none());
        assert!(!cycles.memo_cycles.contains_key(&head_a));
        assert!(!cycles.memo_cycles.contains_key(&head_b));
        assert!(!cycles.memo_cycles.contains_key(&participant));
    }

    #[test]
    fn merge_keeps_participants_current_at_the_merged_iteration() {
        let mut cycle_a = ActiveCycle::new(database_key(0), IterationCount::initial());
        cycle_a.add_participant(database_key(1));

        let mut cycle_b = ActiveCycle::new(database_key(2), IterationCount::initial());
        let next_iteration = IterationCount::initial().increment().unwrap();
        cycle_b.start_next_iteration(next_iteration);
        cycle_b.add_participant(database_key(3));

        cycle_a.merge_from(cycle_b);

        assert_eq!(cycle_a.iteration, next_iteration);
        assert_eq!(
            cycle_a.current_memo_keys(),
            vec![database_key(0), database_key(1), database_key(3)]
        );
    }
}
