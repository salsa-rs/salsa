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
//! Hits update the page-indexed visited bits without taking the state mutex.
//! Admissions, hand movement, and the pending-eviction queue are serialized by
//! that mutex. Selecting a victim removes it from the resident list immediately,
//! but its value is evicted at the next revision only if it was not re-admitted.
//! A separate page-indexed bitmap ensures each id appears in that queue at most
//! once per revision.

use std::num::NonZeroUsize;
use std::ptr;

use crate::Id;
use crate::sync::Mutex;
use crate::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use crate::table::{PAGE_LEN, PAGE_LEN_BITS, split_id};

use super::{EvictionContext, EvictionPolicy, HasCapacity};
use boxcar::buckets::{Buckets, Index, MaybeZeroable, buckets_for_index_bits};

const SLOT_STATE_BITS: usize = 2;
const SLOTS_PER_STATE_WORD: usize = u64::BITS as usize / SLOT_STATE_BITS;
const STATE_WORDS: usize = PAGE_LEN.div_ceil(SLOTS_PER_STATE_WORD);
const SLOTS_PER_PENDING_WORD: usize = u64::BITS as usize;
const PENDING_WORDS: usize = PAGE_LEN.div_ceil(SLOTS_PER_PENDING_WORD);

type PageStates = Buckets<PageSlot, { buckets_for_index_bits(u32::BITS - PAGE_LEN_BITS as u32) }>;
type PageStateIndex = Index<{ buckets_for_index_bits(u32::BITS - PAGE_LEN_BITS as u32) }>;

/// SIEVE eviction policy.
///
/// Values enter at the front of a FIFO queue. Uses set a page-indexed atomic
/// visited bit; they do not move the value in the queue.
pub struct Sieve {
    capacity: Option<NonZeroUsize>,
    state: Mutex<State>,
    page_states: PageStates,
}

impl EvictionPolicy for Sieve {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: NonZeroUsize::new(capacity),
            state: Mutex::default(),
            page_states: PageStates::new(),
        }
    }

    fn record_use(&self, id: Id) {
        if let Some(capacity) = self.capacity {
            if let Some((page, slot)) = page_and_slot_if_allocated(&self.page_states, id) {
                if page.record_use(slot) {
                    return;
                }
            }

            self.record_admission(id, capacity.get());
        }
    }

    fn set_tuning(&mut self, capacity: usize) {
        self.capacity = NonZeroUsize::new(capacity);
        let page_states = &self.page_states;
        let state = self.state.get_mut();
        if let Some(capacity) = self.capacity {
            state.schedule_evictions(capacity.get(), page_states);
        } else {
            for id in state
                .residents
                .resident_ids()
                .chain(state.pending_evictions.iter().copied())
            {
                if let Some((page, slot)) = page_and_slot_if_allocated(page_states, id) {
                    page.clear(slot);
                }
            }
            *state = State::default();
        }
    }

    fn start_new_revision(&mut self, context: &mut impl EvictionContext) {
        let Some(capacity) = self.capacity else {
            return;
        };

        let state = self.state.get_mut();
        for id in state.pending_evictions.drain(..) {
            let (page, slot) = page_and_slot(&self.page_states, id);
            if !page.is_resident(slot) {
                context.evict_value(id);
            }
            page.clear_pending(slot);
        }
        state
            .pending_evictions
            .shrink_to(capacity.get().saturating_mul(2));
    }
}

impl HasCapacity for Sieve {}

impl Sieve {
    fn record_admission(&self, id: Id, capacity: usize) {
        let (page, slot) = page_and_slot_or_alloc(&self.page_states, id);
        let mut state = self.state.lock();

        if page.is_resident(slot) {
            page.record_use(slot);
            return;
        }

        state.insert(id, page, slot, capacity, &self.page_states);
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
    /// The already-resolved location avoids another page-table lookup when the
    /// victim is added to the pending-eviction queue.
    page: &'a PageState,
    slot: usize,
}

impl State {
    fn insert(
        &mut self,
        id: Id,
        page: &PageState,
        slot: usize,
        capacity: usize,
        page_states: &PageStates,
    ) {
        debug_assert!(self.residents.len() <= capacity);
        if self.residents.len() == capacity {
            assert!(
                self.schedule_eviction(page_states),
                "full resident list should have an eviction candidate"
            );
        }

        let node = self.residents.push_front(id);
        page.admit(slot);

        if self.hand == 0 {
            self.hand = node;
        }
    }

    fn schedule_evictions(&mut self, capacity: usize, page_states: &PageStates) {
        self.pending_evictions
            .reserve(self.residents.len().saturating_sub(capacity));
        while self.residents.len() > capacity {
            if !self.schedule_eviction(page_states) {
                return;
            }
        }
    }

    fn schedule_eviction(&mut self, page_states: &PageStates) -> bool {
        let Some(victim) = self.select_victim(page_states) else {
            return false;
        };

        if victim.page.mark_pending(victim.slot) {
            self.pending_evictions.push(victim.id);
        }
        true
    }

    /// Selects and removes the next resident using a bounded SIEVE scan.
    ///
    /// The scan performs at most two inspections per current resident. The
    /// second pass preserves second chances for concurrent re-marks; exhausting
    /// both passes force-evicts at the hand to guarantee progress.
    fn select_victim<'a>(&mut self, page_states: &'a PageStates) -> Option<SelectedVictim<'a>> {
        if self.hand == 0 {
            return None;
        }

        let inspection_budget = self.residents.len().saturating_mul(2);
        for _ in 0..inspection_budget {
            let index = self.hand;
            let id = self.residents.id(index);
            let (page, slot) = page_and_slot(page_states, id);

            if page.select(slot).was_visited() {
                self.hand = self.residents.advance_towards_front(index);
            } else {
                return Some(self.remove_victim(index, page, slot));
            }
        }

        // Every inspected resident was re-marked. Evict at the hand to
        // preserve SIEVE's scan order while guaranteeing progress.
        let index = self.hand;
        let id = self.residents.id(index);
        let (page, slot) = page_and_slot(page_states, id);
        page.clear_resident(slot);
        Some(self.remove_victim(index, page, slot))
    }

    fn remove_victim<'a>(
        &mut self,
        index: ResidentIndex,
        page: &'a PageState,
        slot: usize,
    ) -> SelectedVictim<'a> {
        self.hand = self.residents.hand_after_remove(index);
        SelectedVictim {
            id: self.residents.remove(index),
            page,
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

struct ResidentIds<'a> {
    residents: &'a Residents,
    next: ResidentIndex,
}

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
struct PageSlot {
    ptr: AtomicPtr<PageState>,
}

// SAFETY: `PageSlot`'s all-zero representation is valid because a null
// `AtomicPtr` is valid.
unsafe impl MaybeZeroable for PageSlot {
    fn zeroable() -> bool {
        true
    }
}

impl PageSlot {
    fn get(&self) -> Option<&PageState> {
        let ptr = self.ptr.load(Ordering::Acquire);
        ptr::NonNull::new(ptr).map(|ptr| {
            // SAFETY: A non-null pointer was allocated by `get_or_alloc` and
            // remains owned by this `PageSlot` until it is dropped.
            unsafe { ptr.as_ref() }
        })
    }

    fn get_or_alloc(&self) -> &PageState {
        if let Some(page) = self.get() {
            return page;
        }

        let new_page = Box::into_raw(Box::new(PageState::default()));
        match self.ptr.compare_exchange(
            ptr::null_mut(),
            new_page,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // SAFETY: We just installed this allocation.
                unsafe { &*new_page }
            }
            Err(existing) => {
                // SAFETY: This allocation was not published, so this thread
                // still owns it.
                unsafe { drop(Box::from_raw(new_page)) };

                // SAFETY: `existing` came from a successful install by another
                // thread and remains owned by this `PageSlot`.
                unsafe { &*existing }
            }
        }
    }
}

impl Drop for PageSlot {
    fn drop(&mut self) {
        let ptr = *self.ptr.get_mut();
        if !ptr.is_null() {
            // SAFETY: Dropping the `PageSlot` requires exclusive access, so no
            // more readers can access the pointed-to page.
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

#[derive(Default)]
struct PageState {
    words: [AtomicU64; STATE_WORDS],
    /// Slots that already occur in `State::pending_evictions`.
    ///
    /// These words are only accessed while holding the state mutex or during
    /// an exclusive revision reset. Atomics provide safe interior mutability;
    /// the mutex provides synchronization.
    pending: [AtomicU64; PENDING_WORDS],
}

impl PageState {
    fn record_use(&self, slot: usize) -> bool {
        let (word, resident, visited) = slot_state(slot);
        let word = &self.words[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            if state & resident == 0 {
                return false;
            }

            if state & visited != 0 {
                return true;
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
        let word = &self.words[word];
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
        let word = &self.words[word];
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
        self.words[word].load(Ordering::Relaxed) & resident != 0
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

    fn clear(&self, slot: usize) {
        self.clear_resident(slot);
        self.clear_pending(slot);
    }

    fn clear_resident(&self, slot: usize) {
        let (word, resident, visited) = slot_state(slot);
        self.words[word].fetch_and(!(resident | visited), Ordering::Relaxed);
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

fn page_and_slot_or_alloc(page_states: &PageStates, id: Id) -> (&PageState, usize) {
    let (index, slot) = page_state_index_and_slot(id);
    (page_states.get_or_alloc(index).get_or_alloc(), slot)
}

fn page_and_slot(page_states: &PageStates, id: Id) -> (&PageState, usize) {
    // Resident and pending ids must already have allocated page state.
    page_and_slot_if_allocated(page_states, id).expect("SIEVE page state should be allocated")
}

fn page_and_slot_if_allocated(page_states: &PageStates, id: Id) -> Option<(&PageState, usize)> {
    let (index, slot) = page_state_index_and_slot(id);
    page_states
        .get(index)
        .and_then(PageSlot::get)
        .map(|page| (page, slot))
}

fn page_state_index_and_slot(id: Id) -> (PageStateIndex, usize) {
    let (page, slot) = split_id(id);
    let index = PageStateIndex::new(page.as_usize())
        .expect("page index should fit in SIEVE page state table");
    (index, slot.as_usize())
}

fn slot_state(slot: usize) -> (usize, u64, u64) {
    let bit = (slot % SLOTS_PER_STATE_WORD) * SLOT_STATE_BITS;
    let resident = 1 << bit;
    let visited = 1 << (bit + 1);
    (slot / SLOTS_PER_STATE_WORD, resident, visited)
}

fn pending_state(slot: usize) -> (usize, u64) {
    let bit = slot % SLOTS_PER_PENDING_WORD;
    (slot / SLOTS_PER_PENDING_WORD, 1 << bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestEvictionContext {
        evicted: Vec<Id>,
    }

    impl EvictionContext for TestEvictionContext {
        fn last_verified_at(&mut self, _id: Id) -> Option<crate::Revision> {
            None
        }

        fn evict_value(&mut self, id: Id) {
            self.evicted.push(id);
        }
    }

    fn id(index: u32) -> Id {
        // SAFETY: Test indices are within `Id`'s valid range.
        unsafe { Id::from_index(index) }
    }

    #[test]
    fn all_marked_residents_evict_the_initial_hand() {
        let page_states = PageStates::new();
        let mut state = State::default();
        let oldest = id(0);
        let middle = id(1);
        let newest = id(2);

        for id in [oldest, middle, newest] {
            let (page, slot) = page_and_slot_or_alloc(&page_states, id);
            state.insert(id, page, slot, 3, &page_states);
            assert!(page.record_use(slot));
        }

        assert_eq!(
            state.select_victim(&page_states).map(|victim| victim.id),
            Some(oldest)
        );
        assert_eq!(state.residents.id(state.hand), middle);
        assert_eq!(
            state.residents.resident_ids().collect::<Vec<_>>(),
            [newest, middle]
        );
        let (page, slot) = page_and_slot(&page_states, oldest);
        assert!(!page.is_resident(slot));
    }

    #[test]
    fn insertion_selects_a_victim_before_admitting() {
        let page_states = PageStates::new();
        let mut state = State::default();
        let oldest = id(0);
        let newest = id(1);
        let incoming = id(2);

        for id in [oldest, newest] {
            let (page, slot) = page_and_slot_or_alloc(&page_states, id);
            state.insert(id, page, slot, 2, &page_states);
            assert!(page.record_use(slot));
        }

        let (page, slot) = page_and_slot_or_alloc(&page_states, incoming);
        state.insert(incoming, page, slot, 2, &page_states);

        assert_eq!(state.pending_evictions, [oldest]);
        assert_eq!(state.residents.nodes.len(), 3);
        assert!(page.is_resident(slot));
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

        let mut context = TestEvictionContext::default();
        sieve.start_new_revision(&mut context);

        assert_eq!(context.evicted, [first]);
        assert!(sieve.state.get_mut().pending_evictions.is_empty());

        sieve.record_use(first);
        assert_eq!(sieve.state.lock().pending_evictions, [second]);
    }
}
