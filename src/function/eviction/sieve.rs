//! SIEVE eviction policy.
//!
//! This policy keeps resident values in FIFO order and records a single visited
//! bit for values used after admission. At eviction time, a moving hand gives
//! visited values one second chance by clearing their bit and continuing toward
//! newer entries.

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
            if let Some(page) = self.page_if_allocated(id) {
                if page.record_use(slot_offset(id)) {
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
                if let Some(page) = page_if_allocated(page_states, id) {
                    page.clear(slot_offset(id));
                }
            }
            *state = State::default();
        }
    }

    fn start_new_revision(&mut self, context: &mut impl EvictionContext) {
        if self.capacity.is_none() {
            return;
        }

        let state = self.state.get_mut();
        for id in state.pending_evictions.drain(..) {
            if !page_if_allocated(&self.page_states, id)
                .is_some_and(|page| page.is_resident(slot_offset(id)))
            {
                context.evict_value(id);
            }
        }
    }
}

impl HasCapacity for Sieve {}

impl Sieve {
    fn page_if_allocated(&self, id: Id) -> Option<&PageState> {
        page_if_allocated(&self.page_states, id)
    }

    fn record_admission(&self, id: Id, capacity: usize) {
        let page = page(&self.page_states, id);
        let slot = slot_offset(id);
        let mut state = self.state.lock();

        if page.is_resident(slot) {
            page.record_use(slot);
            return;
        }

        state.insert(id, capacity, &self.page_states);
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

impl State {
    fn insert(&mut self, id: Id, capacity: usize, page_states: &PageStates) {
        let node = self.residents.push_front(id);
        page(page_states, id).admit(slot_offset(id));

        if self.hand == 0 {
            self.hand = node;
        }

        if self.residents.len() > capacity {
            assert!(
                self.schedule_eviction(page_states),
                "resident list should have an eviction candidate after insertion"
            );
        }
    }

    fn schedule_evictions(&mut self, capacity: usize, page_states: &PageStates) {
        while self.residents.len() > capacity {
            if !self.schedule_eviction(page_states) {
                return;
            }
        }
    }

    fn schedule_eviction(&mut self, page_states: &PageStates) -> bool {
        if let Some(victim) = self.select_victim(page_states) {
            self.pending_evictions.push(victim);
            true
        } else {
            false
        }
    }

    fn select_victim(&mut self, page_states: &PageStates) -> Option<Id> {
        loop {
            if self.hand == 0 {
                return None;
            }

            let index = self.hand;
            let id = self.residents.id(index);

            if page(page_states, id).select(slot_offset(id)).was_visited() {
                self.hand = self.residents.advance_towards_front(index);
            } else {
                self.hand = self.residents.hand_after_remove(index);
                return Some(self.residents.remove(index));
            }
        }
    }
}

type ResidentIndex = u32;

struct Residents {
    nodes: Vec<Resident>,
    free: Vec<ResidentIndex>,
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
            free: Vec::new(),
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

        let id = self
            .node_mut(index)
            .id
            .take()
            .expect("resident node should have an id");
        self.node_mut(index).prev = 0;
        self.node_mut(index).next = 0;
        self.free.push(index);
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
        if let Some(index) = self.free.pop() {
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

    fn clear(&self, slot: usize) {
        let (word, resident, visited) = slot_state(slot);
        let word = &self.words[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            let new_state = state & !(resident | visited);
            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(new_state) => state = new_state,
            }
        }
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

fn page(page_states: &PageStates, id: Id) -> &PageState {
    page_states
        .get_or_alloc(page_state_index(id))
        .get_or_alloc()
}

fn page_if_allocated(page_states: &PageStates, id: Id) -> Option<&PageState> {
    page_states
        .get(page_state_index(id))
        .and_then(PageSlot::get)
}

fn page_state_index(id: Id) -> PageStateIndex {
    let (page, _) = split_id(id);
    PageStateIndex::new(page.as_usize()).expect("page index should fit in SIEVE page state table")
}

fn slot_offset(id: Id) -> usize {
    let (_, slot) = split_id(id);
    slot.as_usize()
}

fn slot_state(slot: usize) -> (usize, u64, u64) {
    let bit = (slot % SLOTS_PER_STATE_WORD) * SLOT_STATE_BITS;
    let resident = 1 << bit;
    let visited = 1 << (bit + 1);
    (slot / SLOTS_PER_STATE_WORD, resident, visited)
}
