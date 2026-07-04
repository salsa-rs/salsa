//! SIEVE eviction policy.
//!
//! This policy keeps resident values in FIFO order and records a single visited
//! bit for values used after admission. At eviction time, a moving hand gives
//! visited values one second chance by clearing their bit and continuing toward
//! newer entries.
//!
//! Victim selection is bounded to twice the current resident count. With no
//! concurrent hits, this behaves like textbook SIEVE: if every resident starts
//! visited, the first pass clears those bits and the first resident is selected
//! when the hand reaches it again. A second pass also gives residents re-marked
//! by concurrent hits another chance. If every inspection still finds a visited
//! resident, selection force-evicts the resident at the hand so admissions
//! cannot starve without changing the scan order.
//!
//! Hits update the block-indexed visited bits without taking the state mutex.
//! Admissions, hand movement, and the pending-eviction queue are serialized by
//! that mutex. Selecting a victim removes it from the resident list immediately,
//! but its value is evicted at the next revision only if it was not re-admitted.
//! A separate block-indexed bitmap ensures each id appears in that queue at most
//! once per revision.
//!
//! State is divided into independently allocated blocks. Within each block,
//! interleaved resident/visited bits cover 32 ids per atomic word, while the
//! pending bitmap covers 64 ids per word. This keeps SIEVE's allocation
//! granularity independent of the memo table's page size.

use std::ptr;

use crate::Id;
use crate::sync::Mutex;
use crate::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use super::{EvictionPolicy, HasCapacity};
use boxcar::buckets::{Buckets, Index, MaybeZeroable, buckets_for_index_bits};

const SLOT_STATE_BITS: usize = 2;
const SLOTS_PER_STATE_WORD: usize = u64::BITS as usize / SLOT_STATE_BITS;
const SLOTS_PER_PENDING_WORD: usize = u64::BITS as usize;
const BLOCK_LEN_BITS: usize = 10;
const BLOCK_LEN: usize = 1 << BLOCK_LEN_BITS;
const STATE_WORDS: usize = BLOCK_LEN / SLOTS_PER_STATE_WORD;
const PENDING_WORDS: usize = BLOCK_LEN / SLOTS_PER_PENDING_WORD;

type StateBlocks =
    Buckets<BlockSlot, { buckets_for_index_bits(u32::BITS - BLOCK_LEN_BITS as u32) }>;
type StateBlockIndex = Index<{ buckets_for_index_bits(u32::BITS - BLOCK_LEN_BITS as u32) }>;

/// SIEVE eviction policy.
///
/// Values enter at the front of a FIFO queue. Uses set a block-indexed atomic
/// visited bit; they do not move the value in the queue.
pub struct Sieve {
    capacity: usize,
    state: Mutex<State>,
    state_blocks: StateBlocks,
}

impl EvictionPolicy for Sieve {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::default(),
            state_blocks: StateBlocks::new(),
        }
    }

    #[inline]
    fn record_use(&self, id: Id) {
        // Probe residency before capacity: resident hits do not need the capacity,
        // while admissions already take the cold path below.
        if let Some((block, slot)) = block_and_slot_if_allocated(&self.state_blocks, id) {
            if block.record_use(slot) {
                return;
            }
        }

        if self.capacity != 0 {
            self.record_admission(id, self.capacity);
        }
    }

    fn set_tuning(&mut self, capacity: usize) {
        self.capacity = capacity;
        if capacity == 0 {
            *self.state.get_mut() = State::default();
            self.state_blocks = StateBlocks::new();
        } else {
            self.state
                .get_mut()
                .schedule_evictions(capacity, &self.state_blocks);
        }
    }

    fn for_each_evicted(&mut self, mut evict: impl FnMut(Id)) {
        let capacity = self.capacity;
        if capacity == 0 {
            return;
        }

        let state = self.state.get_mut();
        for id in state.pending_evictions.drain(..) {
            let (block, slot) = block_and_slot(&self.state_blocks, id);
            if !block.is_resident(slot) {
                evict(id);
            }
            block.clear_pending(slot);
        }
        state
            .pending_evictions
            .shrink_to(capacity.saturating_mul(2));
    }
}

impl HasCapacity for Sieve {}

impl Sieve {
    fn record_admission(&self, id: Id, capacity: usize) {
        let (block, slot) = block_and_slot_or_alloc(&self.state_blocks, id);
        let mut state = self.state.lock();

        if block.is_resident(slot) {
            block.record_use(slot);
            return;
        }

        state.insert(id, block, slot, capacity, &self.state_blocks);
    }
}

#[derive(Default)]
struct State {
    residents: Residents,
    /// Index of the next resident candidate to inspect. `0` is the sentinel.
    hand: ResidentIndex,
    /// Victims selected by SIEVE admission, waiting for an eviction context.
    pending_evictions: Vec<Id>,
}

struct SelectedVictim<'a> {
    id: Id,
    /// The already-resolved location avoids another state-directory lookup when the
    /// victim is added to the pending-eviction queue.
    block: &'a StateBlock,
    slot: usize,
}

impl State {
    fn insert(
        &mut self,
        id: Id,
        block: &StateBlock,
        slot: usize,
        capacity: usize,
        state_blocks: &StateBlocks,
    ) {
        debug_assert!(self.residents.len() <= capacity);
        if self.residents.len() == capacity {
            assert!(
                self.schedule_eviction(state_blocks),
                "full resident list should have an eviction candidate"
            );
        }

        let node = self.residents.push_front(id);
        block.admit(slot);

        if self.hand == 0 {
            self.hand = node;
        }
    }

    fn schedule_evictions(&mut self, capacity: usize, state_blocks: &StateBlocks) {
        self.pending_evictions
            .reserve(self.residents.len().saturating_sub(capacity));
        while self.residents.len() > capacity {
            if !self.schedule_eviction(state_blocks) {
                return;
            }
        }
    }

    fn schedule_eviction(&mut self, state_blocks: &StateBlocks) -> bool {
        let Some(victim) = self.select_victim(state_blocks) else {
            return false;
        };

        if victim.block.mark_pending(victim.slot) {
            self.pending_evictions.push(victim.id);
        }
        true
    }

    /// Selects and removes the next resident using a bounded SIEVE scan.
    ///
    /// The scan performs at most two inspections per current resident. The
    /// second pass preserves second chances for concurrent re-marks; exhausting
    /// both passes force-evicts at the hand to guarantee progress.
    fn select_victim<'a>(&mut self, state_blocks: &'a StateBlocks) -> Option<SelectedVictim<'a>> {
        if self.hand == 0 {
            return None;
        }

        let inspection_budget = self.residents.len().saturating_mul(2);
        for _ in 0..inspection_budget {
            let index = self.hand;
            let id = self.residents.id(index);
            let (block, slot) = block_and_slot(state_blocks, id);

            if block.select(slot).was_visited() {
                self.hand = self.residents.advance_towards_front(index);
            } else {
                return Some(self.remove_victim(index, block, slot));
            }
        }

        // Every inspected resident was re-marked. Evict at the hand to
        // preserve SIEVE's scan order while guaranteeing progress.
        let index = self.hand;
        let id = self.residents.id(index);
        let (block, slot) = block_and_slot(state_blocks, id);
        block.clear_resident(slot);
        Some(self.remove_victim(index, block, slot))
    }

    fn remove_victim<'a>(
        &mut self,
        index: ResidentIndex,
        block: &'a StateBlock,
        slot: usize,
    ) -> SelectedVictim<'a> {
        self.hand = self.residents.hand_after_remove(index);
        SelectedVictim {
            id: self.residents.remove(index),
            block,
            slot,
        }
    }
}

type ResidentIndex = u32;

struct Residents {
    nodes: Vec<Resident>,
    /// Intrusive free list linked through `Resident::next`.
    free_head: ResidentIndex,
    len: usize,
}

struct Resident {
    id: Option<Id>,
    /// Newer resident, or the sentinel.
    prev: ResidentIndex,
    /// Older resident, or the sentinel.
    next: ResidentIndex,
}

impl Default for Residents {
    fn default() -> Self {
        Self {
            nodes: vec![Resident {
                id: None,
                prev: 0,
                next: 0,
            }],
            free_head: 0,
            len: 0,
        }
    }
}

impl Residents {
    fn len(&self) -> usize {
        self.len
    }

    fn push_front(&mut self, id: Id) -> ResidentIndex {
        let index = self.alloc_node(id);
        let first = self.node(0).next;

        self.node_mut(index).prev = 0;
        self.node_mut(index).next = first;
        self.node_mut(0).next = index;
        self.node_mut(first).prev = index;

        self.len += 1;
        index
    }

    fn remove(&mut self, index: ResidentIndex) -> Id {
        debug_assert_ne!(index, 0);

        let prev = self.node(index).prev;
        let next = self.node(index).next;
        self.node_mut(prev).next = next;
        self.node_mut(next).prev = prev;
        self.len -= 1;

        let free_head = self.free_head;
        let node = self.node_mut(index);
        let id = node.id.take().expect("resident node should have an id");
        node.prev = 0;
        node.next = free_head;
        self.free_head = index;
        id
    }

    fn id(&self, index: ResidentIndex) -> Id {
        self.node(index)
            .id
            .expect("resident node should have an id")
    }

    fn advance_towards_front(&self, index: ResidentIndex) -> ResidentIndex {
        debug_assert_ne!(index, 0);

        let prev = self.node(index).prev;
        if prev == 0 { self.node(0).prev } else { prev }
    }

    fn hand_after_remove(&self, index: ResidentIndex) -> ResidentIndex {
        debug_assert_ne!(index, 0);

        if self.len == 1 {
            return 0;
        }

        let prev = self.node(index).prev;
        if prev == 0 { self.node(0).prev } else { prev }
    }

    #[cfg(test)]
    fn resident_ids(&self) -> ResidentIds<'_> {
        ResidentIds {
            residents: self,
            next: self.node(0).next,
        }
    }

    fn alloc_node(&mut self, id: Id) -> ResidentIndex {
        if self.free_head != 0 {
            let index = self.free_head;
            self.free_head = self.node(index).next;
            self.node_mut(index).id = Some(id);
            index
        } else {
            let index = ResidentIndex::try_from(self.nodes.len())
                .expect("SIEVE resident capacity should fit in u32");
            self.nodes.push(Resident {
                id: Some(id),
                prev: 0,
                next: 0,
            });
            index
        }
    }

    fn node(&self, index: ResidentIndex) -> &Resident {
        &self.nodes[index as usize]
    }

    fn node_mut(&mut self, index: ResidentIndex) -> &mut Resident {
        &mut self.nodes[index as usize]
    }
}

#[cfg(test)]
struct ResidentIds<'a> {
    residents: &'a Residents,
    next: ResidentIndex,
}

#[cfg(test)]
impl Iterator for ResidentIds<'_> {
    type Item = Id;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == 0 {
            return None;
        }

        let index = self.next;
        self.next = self.residents.node(index).next;
        Some(self.residents.id(index))
    }
}

#[derive(Default)]
struct BlockSlot {
    ptr: AtomicPtr<StateBlock>,
}

// SAFETY: Outside Shuttle, `BlockSlot` contains an atomic pointer whose
// all-zero representation is a valid null pointer. Shuttle atomics require
// construction.
unsafe impl MaybeZeroable for BlockSlot {
    fn zeroable() -> bool {
        cfg!(not(feature = "shuttle"))
    }
}

impl BlockSlot {
    #[inline]
    fn get(&self) -> Option<&StateBlock> {
        let ptr = self.ptr.load(Ordering::Acquire);
        ptr::NonNull::new(ptr).map(|ptr| {
            // SAFETY: A non-null pointer was allocated by `get_or_alloc` and
            // remains owned by this slot until the directory is dropped.
            unsafe { ptr.as_ref() }
        })
    }

    fn get_or_alloc(&self) -> &StateBlock {
        if let Some(block) = self.get() {
            return block;
        }

        let new_block = Box::into_raw(Box::new(StateBlock::default()));
        match self.ptr.compare_exchange(
            ptr::null_mut(),
            new_block,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // SAFETY: We just installed this allocation.
                unsafe { &*new_block }
            }
            Err(existing) => {
                // SAFETY: This allocation was not published, so this thread
                // still owns it.
                unsafe { drop(Box::from_raw(new_block)) };

                // SAFETY: `existing` was installed by another thread and
                // remains owned by this slot.
                unsafe { &*existing }
            }
        }
    }
}

impl Drop for BlockSlot {
    fn drop(&mut self) {
        let ptr = *self.ptr.get_mut();
        if !ptr.is_null() {
            // SAFETY: Dropping the slot requires exclusive access, so no reader
            // can still access the allocation.
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

struct StateBlock {
    /// Interleaved resident and visited bits for all slots in the block.
    state: [AtomicU64; STATE_WORDS],
    /// Slots that already occur in `State::pending_evictions`.
    ///
    /// These words are only accessed while holding the state mutex or during
    /// an exclusive revision reset. Atomics provide safe interior mutability;
    /// the mutex provides synchronization.
    pending: [AtomicU64; PENDING_WORDS],
}

impl Default for StateBlock {
    fn default() -> Self {
        Self {
            state: std::array::from_fn(|_| AtomicU64::default()),
            pending: std::array::from_fn(|_| AtomicU64::default()),
        }
    }
}

impl StateBlock {
    #[inline]
    fn record_use(&self, slot: usize) -> bool {
        let (word, resident, visited) = slot_state(slot);
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            if state & visited != 0 {
                return true;
            }

            if state & resident == 0 {
                return false;
            }

            match word.compare_exchange_weak(
                state,
                state | visited,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new_state) => state = new_state,
            }
        }
    }

    fn admit(&self, slot: usize) {
        let (word, resident, visited) = slot_state(slot);
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            let new_state = (state | resident) & !visited;
            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(new_state) => state = new_state,
            }
        }
    }

    fn select(&self, slot: usize) -> Selection {
        let (word, resident, visited) = slot_state(slot);
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            debug_assert_ne!(
                state & resident,
                0,
                "resident list entry should have a resident state bit"
            );

            let (new_state, selection) = if state & visited == 0 {
                (state & !(resident | visited), Selection::Evict)
            } else {
                (state & !visited, Selection::SecondChance)
            };

            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return selection,
                Err(new_state) => state = new_state,
            }
        }
    }

    fn is_resident(&self, slot: usize) -> bool {
        let (word, resident, _) = slot_state(slot);
        self.state[word].load(Ordering::Relaxed) & resident != 0
    }

    fn mark_pending(&self, slot: usize) -> bool {
        let (word, pending) = pending_state(slot);
        let word = &self.pending[word];
        let state = word.load(Ordering::Relaxed);
        if state & pending != 0 {
            return false;
        }

        word.store(state | pending, Ordering::Relaxed);
        true
    }

    fn clear_pending(&self, slot: usize) {
        let (word, pending) = pending_state(slot);
        let word = &self.pending[word];
        let state = word.load(Ordering::Relaxed);
        word.store(state & !pending, Ordering::Relaxed);
    }

    fn clear_resident(&self, slot: usize) {
        let (word, resident, visited) = slot_state(slot);
        self.state[word].fetch_and(!(resident | visited), Ordering::Relaxed);
    }
}

#[derive(Copy, Clone)]
enum Selection {
    SecondChance,
    Evict,
}

impl Selection {
    fn was_visited(self) -> bool {
        matches!(self, Selection::SecondChance)
    }
}

fn block_and_slot_or_alloc(state_blocks: &StateBlocks, id: Id) -> (&StateBlock, usize) {
    let (index, slot) = state_block_index_and_slot(id);
    (state_blocks.get_or_alloc(index).get_or_alloc(), slot)
}

fn block_and_slot(state_blocks: &StateBlocks, id: Id) -> (&StateBlock, usize) {
    // Resident and pending ids must already have allocated state blocks.
    block_and_slot_if_allocated(state_blocks, id).expect("SIEVE state block should be allocated")
}

#[inline]
fn block_and_slot_if_allocated(state_blocks: &StateBlocks, id: Id) -> Option<(&StateBlock, usize)> {
    let (index, slot) = state_block_index_and_slot(id);
    state_blocks
        .get(index)
        .and_then(BlockSlot::get)
        .map(|block| (block, slot))
}

#[inline]
fn state_block_index_and_slot(id: Id) -> (StateBlockIndex, usize) {
    let index = id.index() as usize;
    let block = index >> BLOCK_LEN_BITS;
    let slot = index & (BLOCK_LEN - 1);
    // SAFETY: `Id` is a `u32`, and `StateBlocks` has enough buckets for every
    // block representable by an `Id` after removing the slot bits.
    let index = unsafe { StateBlockIndex::new_unchecked(block) };
    (index, slot)
}

fn slot_state(slot: usize) -> (usize, u64, u64) {
    debug_assert!(slot < BLOCK_LEN);
    let bit = (slot % SLOTS_PER_STATE_WORD) * SLOT_STATE_BITS;
    let resident = 1 << bit;
    let visited = 1 << (bit + 1);
    (slot / SLOTS_PER_STATE_WORD, resident, visited)
}

fn pending_state(slot: usize) -> (usize, u64) {
    debug_assert!(slot < BLOCK_LEN);
    let bit = slot % SLOTS_PER_PENDING_WORD;
    (slot / SLOTS_PER_PENDING_WORD, 1 << bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> Id {
        // SAFETY: Test indices are within `Id`'s valid range.
        unsafe { Id::from_index(index) }
    }

    #[test]
    fn all_marked_residents_evict_the_initial_hand() {
        let state_blocks = StateBlocks::new();
        let mut state = State::default();
        let oldest = id(0);
        let middle = id(1);
        let newest = id(2);

        for id in [oldest, middle, newest] {
            let (block, slot) = block_and_slot_or_alloc(&state_blocks, id);
            state.insert(id, block, slot, 3, &state_blocks);
            assert!(block.record_use(slot));
        }

        assert_eq!(
            state.select_victim(&state_blocks).map(|victim| victim.id),
            Some(oldest)
        );
        assert_eq!(state.residents.id(state.hand), middle);
        assert_eq!(
            state.residents.resident_ids().collect::<Vec<_>>(),
            [newest, middle]
        );
        let (block, slot) = block_and_slot(&state_blocks, oldest);
        assert!(!block.is_resident(slot));
    }

    #[test]
    fn insertion_selects_a_victim_before_admitting() {
        let state_blocks = StateBlocks::new();
        let mut state = State::default();
        let oldest = id(0);
        let newest = id(1);
        let incoming = id(2);

        for id in [oldest, newest] {
            let (block, slot) = block_and_slot_or_alloc(&state_blocks, id);
            state.insert(id, block, slot, 2, &state_blocks);
            assert!(block.record_use(slot));
        }

        let (block, slot) = block_and_slot_or_alloc(&state_blocks, incoming);
        state.insert(incoming, block, slot, 2, &state_blocks);

        assert_eq!(state.pending_evictions, [oldest]);
        assert_eq!(state.residents.nodes.len(), 3);
        assert!(block.is_resident(slot));
    }

    #[test]
    fn disabling_discards_state_blocks() {
        let mut sieve = Sieve::new(1);
        let resident = id(0);

        sieve.record_use(resident);
        assert!(block_and_slot_if_allocated(&sieve.state_blocks, resident).is_some());

        sieve.set_tuning(0);
        assert!(block_and_slot_if_allocated(&sieve.state_blocks, resident).is_none());
        assert_eq!(sieve.state.get_mut().residents.len(), 0);

        sieve.record_use(resident);
        assert!(block_and_slot_if_allocated(&sieve.state_blocks, resident).is_none());
    }

    #[test]
    fn pending_evictions_are_deduplicated_until_reset() {
        let mut sieve = Sieve::new(1);
        let first = id(0);
        let second = id(1);

        for _ in 0..100 {
            sieve.record_use(first);
            sieve.record_use(second);
        }

        assert_eq!(sieve.state.lock().pending_evictions, [first, second]);

        let mut evicted = Vec::new();
        sieve.for_each_evicted(|id| evicted.push(id));

        assert_eq!(evicted, [first]);
        assert!(sieve.state.get_mut().pending_evictions.is_empty());

        sieve.record_use(first);
        assert_eq!(sieve.state.lock().pending_evictions, [second]);
    }
}
