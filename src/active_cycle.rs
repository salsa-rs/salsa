use std::num::NonZeroUsize;

use crate::cycle::{CycleHeads, IterationCount};
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
    memo_states: FxHashMap<DatabaseKeyIndex, ActiveCycleMemoState>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveCycleMemoState {
    pub(crate) cycle_heads: CycleHeads,
    pub(crate) iteration: IterationCount,
}

impl ActiveCycle {
    fn new(head: DatabaseKeyIndex, iteration: IterationCount) -> Self {
        Self {
            converged: false,
            memo_states: FxHashMap::from_iter([(
                head,
                ActiveCycleMemoState {
                    cycle_heads: CycleHeads::initial(head, iteration),
                    iteration,
                },
            )]),
        }
    }

    fn set_memo_state(
        &mut self,
        memo: DatabaseKeyIndex,
        cycle_heads: CycleHeads,
        iteration: IterationCount,
    ) {
        self.memo_states.insert(
            memo,
            ActiveCycleMemoState {
                cycle_heads,
                iteration,
            },
        );
    }

    fn memo_state(&self, memo: DatabaseKeyIndex) -> Option<ActiveCycleMemoState> {
        self.memo_states.get(&memo).cloned()
    }

    fn memo_state_mut(&mut self, memo: DatabaseKeyIndex) -> Option<&mut ActiveCycleMemoState> {
        self.memo_states.get_mut(&memo)
    }

    fn set_converged(&mut self, converged: bool) {
        self.converged = converged;
    }
}

#[derive(Debug)]
struct ActiveCycleSlot {
    generation: usize,
    cycle: Option<ActiveCycle>,
}

#[derive(Debug, Default)]
pub(crate) struct ActiveCycles {
    slots: Vec<ActiveCycleSlot>,
    free: Vec<usize>,
    memo_cycles: FxHashMap<DatabaseKeyIndex, ActiveCycleKey>,
    completed: FxHashMap<ActiveCycleKey, ActiveCycle>,
}

impl ActiveCycles {
    pub(crate) fn insert(
        &mut self,
        head: DatabaseKeyIndex,
        iteration: IterationCount,
    ) -> ActiveCycleKey {
        let key = if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index];
            slot.generation = slot
                .generation
                .checked_add(1)
                .expect("active cycle generation overflow");
            if slot.generation > INDEX_MASK {
                panic!("active cycle generation overflow");
            }
            slot.cycle = Some(ActiveCycle::new(head, iteration));
            ActiveCycleKey::new(index, slot.generation)
        } else {
            let index = self.slots.len();
            assert!(index < INDEX_MASK, "too many active cycles");
            let generation = 1;
            self.slots.push(ActiveCycleSlot {
                generation,
                cycle: Some(ActiveCycle::new(head, iteration)),
            });
            ActiveCycleKey::new(index, generation)
        };

        self.memo_cycles.insert(head, key);
        key
    }

    pub(crate) fn remove(&mut self, key: ActiveCycleKey) -> Option<ActiveCycle> {
        self.remove_active(key, false)
    }

    fn remove_active(
        &mut self,
        key: ActiveCycleKey,
        retain_memo_mappings: bool,
    ) -> Option<ActiveCycle> {
        let index = key.index();
        let slot = self.slots.get_mut(index)?;
        if slot.generation != key.generation() {
            return None;
        }

        let cycle = slot.cycle.take()?;
        self.free.push(index);
        if !retain_memo_mappings {
            for memo in cycle.memo_states.keys() {
                if self.memo_cycles.get(memo) == Some(&key) {
                    self.memo_cycles.remove(memo);
                }
            }
        }
        Some(cycle)
    }

    pub(crate) fn finish(&mut self, key: ActiveCycleKey) -> Option<()> {
        let cycle = self.remove_active(key, true)?;
        self.completed.insert(key, cycle);
        Some(())
    }

    pub(crate) fn get(&self, key: ActiveCycleKey) -> Option<&ActiveCycle> {
        if let Some(cycle) = self.completed.get(&key) {
            return Some(cycle);
        }

        let slot = self.slots.get(key.index())?;
        if slot.generation == key.generation() {
            slot.cycle.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn get_mut(&mut self, key: ActiveCycleKey) -> Option<&mut ActiveCycle> {
        if let Some(cycle) = self.completed.get_mut(&key) {
            return Some(cycle);
        }

        let slot = self.slots.get_mut(key.index())?;
        if slot.generation == key.generation() {
            slot.cycle.as_mut()
        } else {
            None
        }
    }

    pub(crate) fn clear_completed(&mut self) {
        for (memo, active_cycle) in std::mem::take(&mut self.memo_cycles) {
            if self.completed.contains_key(&active_cycle) {
                continue;
            }
            self.memo_cycles.insert(memo, active_cycle);
        }
        self.completed.clear();
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

    pub(crate) fn finish(&self, key: ActiveCycleKey) -> Option<()> {
        self.0.lock().finish(key)
    }

    pub(crate) fn converged(&self, key: ActiveCycleKey) -> Option<bool> {
        self.0.lock().get(key).map(|cycle| cycle.converged)
    }

    pub(crate) fn clear_completed(&self) {
        self.0.lock().clear_completed();
    }

    pub(crate) fn set_memo_state(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
        cycle_heads: CycleHeads,
        iteration: IterationCount,
    ) -> Option<()> {
        let mut cycles = self.0.lock();
        let cycle = cycles.get_mut(key)?;
        cycle.set_memo_state(memo, cycle_heads, iteration);
        cycles.memo_cycles.insert(memo, key);
        Some(())
    }

    pub(crate) fn memo_state_for(
        &self,
        memo: DatabaseKeyIndex,
    ) -> Option<(ActiveCycleKey, ActiveCycleMemoState)> {
        let cycles = self.0.lock();
        let key = *cycles.memo_cycles.get(&memo)?;
        cycles
            .get(key)
            .and_then(|cycle| cycle.memo_state(memo))
            .map(|state| (key, state))
    }

    pub(crate) fn key_for(&self, memo: DatabaseKeyIndex) -> Option<ActiveCycleKey> {
        self.0.lock().memo_cycles.get(&memo).copied()
    }

    pub(crate) fn with_memo_state_mut<R>(
        &self,
        key: ActiveCycleKey,
        memo: DatabaseKeyIndex,
        f: impl FnOnce(&mut ActiveCycleMemoState) -> R,
    ) -> Option<R> {
        let mut cycles = self.0.lock();
        let cycle = cycles.get_mut(key)?;
        cycle.memo_state_mut(memo).map(f)
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
