//! An implementation of the SIEVE cache eviction algorithm described in the
//! [NSDI '24 paper]. The [SIEVE project website] provides a visual explanation.
//!
//! # How SIEVE works
//!
//! SIEVE keeps keys in a queue in the order they were added. A key is resident
//! while it is in the queue. Each resident key has a `visited` bit. The hand
//! points to the next key to inspect, starting with the oldest key.
//!
//! - New residents enter at the newest end with `visited = false`.
//! - Reading a resident key sets `visited = true` without moving the key.
//! - When space is needed, the hand starts at its current position and inspects
//!   residents toward the newest end. It stops as soon as it evicts an
//!   unvisited resident. A visited resident instead gets a second chance: its
//!   visited bit is cleared and the hand advances to the next newer resident.
//! - The next time space is needed, the hand resumes at the resident after the
//!   previous victim. Across eviction requests, it therefore moves toward newer
//!   residents, wrapping to the oldest after passing the newest.
//!
//! For example, `*` marks a visited resident:
//!
//! ```text
//! newest                         oldest
//!   [D]      [C*]      [B]      [A*]
//!                                  ^ hand
//!
//! inspect A*: clear its visited bit, advance to B
//! inspect B:  B is unvisited, so evict B
//! next hand:  C*
//! ```
//!
//! The visited bit records whether the key was read after it was added to the
//! queue or after the hand last cleared the bit.
//!
//! ## Bounded selection
//!
//! Concurrent reads can set visited bits again while SIEVE scans the queue. To
//! avoid holding the mutex indefinitely, SIEVE limits the scan to two passes. If
//! it still has not selected a key, it selects the next key even if it is visited.
//!
//! # Implementation
//!
//! SIEVE separates the state needed for reads from the state needed to manage
//! the queue. This lets threads read resident keys without taking a mutex.
//!
//! `state_pages` stores the atomic resident and visited bits. The state mutex
//! protects the queue and pending evictions.
//!
//! The queue is split into two deques at the hand. `passed` contains the
//! older keys that the hand has already inspected. `candidates` contains
//! the keys from the hand to the newest key. Both deques store keys from
//! oldest to newest. When `candidates` is empty, SIEVE swaps the deques to
//! wrap the hand.
//!
//! The resulting layout is:
//!
//! ```text
//! Sieve
//! ├── capacity
//! ├── state_pages: StatePages         lock-free hit state
//! │   └── StatePageCell               lazily owns one StatePage allocation
//! │       └── StatePage               state for 1,024 table slots
//! │           ├── resident/visited bits
//! │           └── pending-page queue index
//! └── state: CachePadded<Mutex<State>> admission and eviction state
//!     ├── passed: VecDeque<u32>       residents before the hand
//!     ├── candidates: VecDeque<u32>   residents from the hand to newest
//!     └── pending_pages: Vec<PendingPage>
//!         └── 1,024-bit bitmap        selected slots awaiting validation
//! ```
//!
//! On a read, SIEVE looks up the [`StatePage`] for the key. If the key is
//! resident, SIEVE sets its visited bit atomically and returns. If the key
//! is not resident, or is removed before the read can mark it, SIEVE takes
//! the state mutex to add it to the queue.
//!
//! State pages must remain valid while other threads access them. Reads and
//! eviction can update the same bits concurrently, so the bit updates must
//! also be atomic.
//!
//! ## State-page mapping without a hash map
//!
//! SIEVE uses [`Id::index`] to find the resident and visited bits without
//! hashing. Each state page covers 1,024 table slots and uses 256 bytes for
//! these bits. Pages are allocated on first use.
//!
//! ## Deferred eviction
//!
//! When the queue is full, SIEVE selects a key for eviction to make room
//! for the next key. Selecting keys immediately is important for correctness.
//! If SIEVE waited until the next sweep, keys could receive additional
//! second chances, changing which values survive. It also keeps the queue
//! within its configured capacity.
//!
//! SIEVE removes the selected key from the queue and clears its resident
//! bit. However, Salsa cannot immediately drop the memo value because
//! queries may still hold references to it. Instead, SIEVE records the key
//! for eviction at the next sweep.
//!
//! To keep the memory overhead small, SIEVE groups pending evictions by
//! state page. Each [`PendingPage`] stores a page number and a 1,024-bit
//! bitmap, with one bit for each slot selected for eviction. The state page
//! stores an index into `State::pending_pages`, allowing SIEVE to update
//! the bitmap directly when selecting another key from the same page.
//!
//! Between selection and the sweep, a query may read the selected key
//! again. The memo value is still available, so SIEVE can reuse it and
//! add the key back to the queue. Its pending bit remains set. During
//! the sweep, SIEVE therefore checks the pending bitmap against the
//! resident bits and only evicts values whose keys are still absent
//! from the queue.
//!
//! ## Synchronization
//!
//! SIEVE uses a mutex to protect the queue and select keys for eviction.
//! Reads of resident keys do not take this mutex. Instead, they use atomic
//! operations to set the visited bit. This allows reads and eviction to
//! run concurrently.
//!
//! Each key has a resident bit and a visited bit in the same atomic word.
//! A read can only set the visited bit while the resident bit is set.
//! If eviction clears the resident bit first, the read must acquire the
//! mutex to add the key back to the queue. This ensures that a concurrent
//! read cannot mark a key as visited after it has been removed.
//!
//! The bits use relaxed memory ordering because they only track eviction
//! state. Salsa synchronizes access to the memo values separately.
//!
//! State pages are allocated on demand and shared between threads.
//! [`StatePageCell`] uses release ordering when publishing a new page
//! and acquire ordering when reading its pointer. This ensures that
//! readers see a fully initialized page. Once allocated, a page remains
//! valid until SIEVE is disabled or dropped.
//!
//! [NSDI '24 paper]: https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo
//! [SIEVE project website]: https://cachemon.github.io/SIEVE-website/

use std::collections::VecDeque;
use std::ptr;

use crate::Id;
use crate::sync::Mutex;
use crate::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};

use super::{EvictionPolicy, HasCapacity};
use boxcar::buckets::{Buckets, Index, MaybeZeroable, buckets_for_index_bits};
use crossbeam_utils::CachePadded;

/// SIEVE eviction policy.
///
/// New entries join the queue with their visited bit clear. A cache hit
/// atomically sets that bit without moving the entry in the queue.
pub struct Sieve {
    /// Maximum number of residents. Zero disables SIEVE entirely.
    ///
    /// Mutation requires `&mut self`, so hit paths can read this without an
    /// atomic or the state mutex.
    capacity: usize,

    /// Maps table slots to pages containing resident and visited bits.
    ///
    /// Once allocated, pages stay at fixed addresses so concurrent readers
    /// can safely access them. Disabling SIEVE frees all pages.
    state_pages: StatePages,

    /// Tracks the keys in insertion order, the current position of the hand,
    /// and the keys selected for eviction at the next sweep.
    ///
    /// Padding keeps queue updates from invalidating the cache lines that
    /// resident hits read for `capacity` and `state_pages`.
    state: CachePadded<Mutex<State>>,
}

impl EvictionPolicy for Sieve {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state_pages: StatePages::new(),
            state: CachePadded::new(Mutex::new(State::with_capacity(capacity))),
        }
    }

    #[inline]
    fn record_use(&self, id: Id) {
        if self.capacity == 0 {
            return;
        }
        let (number, offset) = state_page_number_and_offset(id.index());
        let page = self.state_pages.get(number);
        if let Some(page) = page
            && page.record_use(offset)
        {
            return;
        }

        self.record_admission(id, page);
    }

    fn for_each_evicted(&mut self, mut evict: impl FnMut(Id)) {
        let capacity = self.capacity;
        if capacity == 0 {
            return;
        }

        let state = self.state.get_mut();
        // Pop from the end so the remaining pages keep their indexes if
        // dropping a memo panics. Unfinished evictions on this page are lost.
        while let Some(pending_page) = state.pending_pages.pop() {
            let page = &self.state_pages[pending_page.page_number];
            page.clear_pending_page_index(state.pending_pages.len());

            pending_page.for_each_evicted_offset(page, |offset| {
                evict(slot_id(pending_page.page_number, offset));
            });
        }

        // Sparse ids can temporarily queue one page per resident. Once empty,
        // retain only a small amount of storage relative to a dense cache.
        state
            .pending_pages
            .shrink_to(capacity.div_ceil(STATE_PAGE_LEN).saturating_mul(2));
    }
}

impl HasCapacity for Sieve {
    fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        if capacity == 0 {
            *self.state.get_mut() = State::default();
            self.state_pages = StatePages::new();
        } else {
            let state = self.state.get_mut();
            state.reserve_capacity(capacity);
            while state.len() > capacity {
                state.schedule_eviction(&self.state_pages);
            }
        }
    }
}

impl Sieve {
    fn record_admission(&self, id: Id, page: Option<&StatePage>) {
        let (number, offset) = state_page_number_and_offset(id.index());
        let page = page.unwrap_or_else(|| self.state_pages.get_or_alloc(number));
        let mut state = self.state.lock();

        // Another thread may have admitted the slot before we acquired the mutex.
        if page.record_use(offset) {
            return;
        }

        debug_assert!(state.len() <= self.capacity);
        if state.len() == self.capacity {
            state.schedule_eviction(&self.state_pages);
        }

        state.candidates.push_back(id.index());
        page.admit(offset);
    }
}

/// Tracks the queue, the hand, and pending evictions.
///
/// The following invariants hold between mutations.
///
/// - The queue consists of `passed` followed by `candidates`, ordered
///   from oldest to newest. The hand points to the first key in `candidates`.
/// - Every resident slot appears exactly once in either `passed` or
///   `candidates`. Every slot in these deques has its resident bit set.
/// - Each state page appears at most once in `pending_pages`. Its
///   `pending_page_index` stores its position in the vector plus one.
///   Zero means the page has no entry in the vector.
#[derive(Default)]
struct State {
    /// Slots given a second chance since the last wrap, oldest first.
    passed: VecDeque<u32>,
    /// Slots from the hand to the newest admission, oldest first.
    candidates: VecDeque<u32>,
    /// Stores one entry per page, with a bitmap of slots selected for eviction.
    /// This reduces memory usage and lets the sweep look up each page only once.
    pending_pages: Vec<PendingPage>,
}

impl State {
    fn with_capacity(capacity: usize) -> Self {
        // Either deque can hold all residents. Reserve both to avoid growing
        // a buffer under the admission mutex when the hand crosses a hot set.
        Self {
            passed: VecDeque::with_capacity(capacity),
            candidates: VecDeque::with_capacity(capacity),
            pending_pages: Vec::new(),
        }
    }

    fn reserve_capacity(&mut self, capacity: usize) {
        self.passed
            .reserve_exact(capacity.saturating_sub(self.passed.len()));
        self.candidates
            .reserve_exact(capacity.saturating_sub(self.candidates.len()));
    }

    fn len(&self) -> usize {
        self.passed.len() + self.candidates.len()
    }

    fn schedule_eviction(&mut self, state_pages: &StatePages) {
        // Allow two passes in case concurrent reads set visited bits again.
        // Then force an eviction to avoid scanning indefinitely.
        let mut budget = self.len().saturating_mul(2);
        loop {
            let slot = self
                .candidates
                .pop_front()
                .expect("non-empty resident queue");
            let (number, offset) = state_page_number_and_offset(slot);
            let page = &state_pages[number];

            let keep = if budget == 0 {
                std::hint::cold_path();
                // Concurrent hits re-marked every candidate. Force progress.
                page.clear_resident(offset);
                false
            } else {
                budget -= 1;
                page.grant_second_chance(offset)
            };

            if keep {
                self.passed.push_back(slot);
            }

            // Wrap immediately, even after removing the newest resident.
            // Otherwise an incoming slot would jump ahead of the survivors.
            if self.candidates.is_empty() {
                std::mem::swap(&mut self.passed, &mut self.candidates);
            }

            if !keep {
                self.mark_pending(number, page, offset);
                return;
            }
        }
    }

    fn mark_pending(&mut self, number: StatePageNumber, page: &StatePage, offset: StatePageOffset) {
        let queue_index = page.pending_page_index().unwrap_or_else(|| {
            let index = self.pending_pages.len();
            self.pending_pages.push(PendingPage::new(number));
            page.set_pending_page_index(index);
            index
        });
        let pending = &mut self.pending_pages[queue_index];
        debug_assert_eq!(pending.page_number, number);
        pending.mark(offset);
    }
}

const SLOTS_PER_PENDING_WORD: usize = u64::BITS as usize;
const PENDING_WORDS: usize = STATE_PAGE_LEN / SLOTS_PER_PENDING_WORD;

/// Tracks slots selected for eviction from one [`StatePage`].
///
/// Each set bit in `pending` identifies a slot selected for eviction.
/// Adding a key back to the queue sets its resident bit. The sweep
/// checks this bit and keeps the memo value if it is set.
struct PendingPage {
    /// The page containing the selected slots.
    page_number: StatePageNumber,
    /// A bitmap of slots selected for eviction.
    pending: [u64; PENDING_WORDS],
}

impl PendingPage {
    #[inline]
    fn new(page_number: StatePageNumber) -> Self {
        Self {
            page_number,
            pending: [0; PENDING_WORDS],
        }
    }

    #[inline]
    fn mark(&mut self, offset: StatePageOffset) {
        let (word, pending_mask) = offset.pending_word();
        self.pending[word] |= pending_mask;
    }

    /// Visits non-resident pending slots in ascending page-offset order.
    ///
    /// This intentionally uses a callback instead of an iterator. An iterator
    /// must preserve the current word and word index across calls to `next`;
    /// measured iterator versions increased `fill_and_evict` instructions.
    fn for_each_evicted_offset(&self, page: &StatePage, mut f: impl FnMut(StatePageOffset)) {
        for (word_index, &pending) in self.pending.iter().enumerate() {
            if pending == 0 {
                continue;
            }

            // The drain has exclusive access to the policy. A pending slot
            // that was re-admitted must keep its memo.
            let mut pending = pending & !page.resident_bits(word_index);
            while pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                let offset = StatePageOffset::new(word_index * SLOTS_PER_PENDING_WORD + bit);
                f(offset);
                pending &= pending - 1;
            }
        }
    }
}

/// Number of low slot-index bits used for the offset within a state page.
const STATE_PAGE_LEN_BITS: usize = 10;
/// Number of table slots represented by one independently allocated state page.
const STATE_PAGE_LEN: usize = 1 << STATE_PAGE_LEN_BITS;

/// Number of Boxcar buckets needed to address every state page.
///
/// State-page numbers use 22 bits. Because Boxcar skips its smallest
/// buckets, `buckets_for_index_bits(22)` leaves 32 page numbers uncovered.
/// One additional bucket is needed to support the full range.
const STATE_PAGE_BUCKETS: usize =
    buckets_for_index_bits(u32::BITS - STATE_PAGE_LEN_BITS as u32) + 1;

/// State-page number formed from the high bits of [`Id::index`].
type StatePageNumber = Index<STATE_PAGE_BUCKETS>;

/// Maps state-page numbers to [`StatePageCell`] entries using Boxcar buckets.
///
/// Buckets and state pages are allocated as needed. Once allocated, pages
/// remain at fixed addresses so concurrent readers can safely access them
/// without taking a mutex.
struct StatePages(Buckets<StatePageCell, STATE_PAGE_BUCKETS>);

impl StatePages {
    fn new() -> Self {
        Self(Buckets::new())
    }

    /// Returns the state page, allocating it if needed.
    ///
    /// Concurrent callers may race to allocate the same page; all receive the
    /// single allocation published by its [`StatePageCell`].
    ///
    /// # Panics
    ///
    /// May panic if allocating the directory bucket or state page fails.
    fn get_or_alloc(&self, number: StatePageNumber) -> &StatePage {
        self.0.get_or_alloc(number).get_or_alloc()
    }

    #[inline]
    fn get(&self, page_number: StatePageNumber) -> Option<&StatePage> {
        self.0.get(page_number).and_then(StatePageCell::get)
    }
}

impl std::ops::Index<StatePageNumber> for StatePages {
    type Output = StatePage;

    fn index(&self, page_number: StatePageNumber) -> &Self::Output {
        self.get(page_number)
            .expect("SIEVE state page should be allocated")
    }
}

/// A slot in a Boxcar bucket that holds a pointer to one [`StatePage`].
///
/// The pointer is initially null. A page is allocated when an ID first
/// needs it. Other slots in the bucket remain empty until used.
/// If multiple threads allocate the same page concurrently, only one
/// allocation is kept and shared by all threads.
#[derive(Default)]
struct StatePageCell {
    /// Null before initialization; otherwise owns the published allocation.
    ptr: AtomicPtr<StatePage>,
}

// SAFETY: Outside Shuttle, `StatePageCell` contains an atomic pointer whose
// all-zero representation is a valid null pointer. Shuttle atomics require
// construction.
unsafe impl MaybeZeroable for StatePageCell {
    fn zeroable() -> bool {
        cfg!(not(feature = "shuttle"))
    }
}

impl StatePageCell {
    #[inline]
    fn get(&self) -> Option<&StatePage> {
        let ptr = self.ptr.load(Ordering::Acquire);
        ptr::NonNull::new(ptr).map(|ptr| {
            // SAFETY: `Self::get_or_alloc` initializes the pointer with a valid
            // allocation. The cell owns that allocation until it is dropped,
            // and the returned reference cannot outlive the cell.
            unsafe { ptr.as_ref() }
        })
    }

    fn get_or_alloc(&self) -> &StatePage {
        if let Some(page) = self.get() {
            return page;
        }

        let new_page = Box::into_raw(Box::new(StatePage::default()));
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

                // SAFETY: `existing` was installed by another thread and
                // remains owned by this slot.
                unsafe { &*existing }
            }
        }
    }
}

impl Drop for StatePageCell {
    fn drop(&mut self) {
        let ptr = *self.ptr.get_mut();
        if !ptr.is_null() {
            // SAFETY: Dropping the slot requires exclusive access, so no reader
            // can still access the allocation.
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

/// Resident and visited bits stored for each id in a [`StatePage`].
const SLOT_STATE_BITS: usize = 2;
/// Number of ids represented by one resident/visited word.
const SLOTS_PER_STATE_WORD: usize = u64::BITS as usize / SLOT_STATE_BITS;
const STATE_WORDS: usize = STATE_PAGE_LEN / SLOTS_PER_STATE_WORD;

/// Stores eviction state for [`STATE_PAGE_LEN`] consecutive table slots.
///
/// Each slot has a resident bit and a visited bit in the same atomic word.
/// This ensures that a concurrent read can only mark a slot as visited
/// while it is still resident. If eviction removes the slot first, the
/// read must add it back to the queue.
#[derive(Default)]
struct StatePage {
    /// Each word holds 32 visited bits in the low half and the corresponding
    /// resident bits in the high half.
    ///
    /// If a slot's visited bit is set, its resident bit is also set.
    state: [AtomicU64; STATE_WORDS],
    /// Position in `State::pending_pages`, counting from one.
    /// Zero means the page has no pending entry.
    ///
    /// Updates must keep this index consistent with `State::pending_pages`.
    /// The state mutex prevents concurrent admissions from adding the same
    /// page twice. Code with `&mut Sieve`, such as the sweep, can update
    /// both without locking because no other thread can access the policy.
    ///
    /// The field is atomic because state pages are shared between threads.
    pending_page_index: AtomicU32,
}

impl StatePage {
    /// Marks the slot as visited if it is resident.
    ///
    /// Returns `false` if the slot needs to be admitted.
    #[inline]
    fn record_use(&self, offset: StatePageOffset) -> bool {
        let (word, resident_mask, visited_mask) = offset.state_word();
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        // Most hits are already visited. Only unvisited slots need the
        // resident mask and compare-and-exchange loop.
        if state as u32 & visited_mask as u32 != 0 {
            return true;
        }

        loop {
            // Not resident. The caller must return to the admission path.
            if state & resident_mask == 0 {
                return false;
            }

            // Set the visited bit. If another thread changed the word,
            // check whether the slot is still resident or already visited.
            match word.compare_exchange_weak(
                state,
                state | visited_mask,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new_state) if new_state & visited_mask != 0 => return true,
                Err(new_state) => state = new_state,
            }
        }
    }

    /// Marks a slot resident and unvisited, including a pending victim being
    /// re-admitted before its memo value is deleted.
    fn admit(&self, offset: StatePageOffset) {
        let (word, resident_mask, visited_mask) = offset.state_word();
        // We checked that the slot was absent under the admission mutex.
        // A concurrent hit can only mark a resident slot, so both bits are
        // still clear. Setting just the resident bit preserves neighboring slots.
        let old = self.state[word].fetch_or(resident_mask, Ordering::Relaxed);
        debug_assert_eq!(old & (resident_mask | visited_mask), 0);
    }

    /// Atomically grants a second chance or makes an unvisited slot
    /// non-resident and eligible for pending eviction.
    fn grant_second_chance(&self, offset: StatePageOffset) -> bool {
        let (word, resident_mask, visited_mask) = offset.state_word();
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            debug_assert_ne!(
                state & resident_mask,
                0,
                "queued slot should have a resident state bit"
            );

            let visited = state & visited_mask != 0;
            if visited {
                // Only selection can clear this bit, and we hold the admission
                // mutex. Changes to neighboring slots need not trigger a retry.
                word.fetch_and(!visited_mask, Ordering::Relaxed);
                return true;
            }
            let new_state = state & !resident_mask;
            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return false,
                Err(new_state) => state = new_state,
            }
        }
    }

    /// Combines the resident bits from two state words into
    /// a bitmap that matches the layout of `PendingPage::pending`.
    fn resident_bits(&self, pending_word: usize) -> u64 {
        let lo = self.state[2 * pending_word].load(Ordering::Relaxed);
        let hi = self.state[2 * pending_word + 1].load(Ordering::Relaxed);
        (lo >> 32) | (hi & (u64::MAX << 32))
    }

    #[inline]
    fn pending_page_index(&self) -> Option<usize> {
        match self.pending_page_index.load(Ordering::Relaxed) {
            0 => None,
            index => Some((index - 1) as usize),
        }
    }

    #[inline]
    fn set_pending_page_index(&self, index: usize) {
        debug_assert!(self.pending_page_index().is_none());
        let index = u32::try_from(index + 1).expect("pending page index should fit in u32");
        self.pending_page_index.store(index, Ordering::Relaxed);
    }

    fn clear_pending_page_index(&self, index: usize) {
        debug_assert_eq!(self.pending_page_index(), Some(index));
        self.pending_page_index.store(0, Ordering::Relaxed);
    }

    /// Atomically marks the slot at `offset` absent by clearing its resident and
    /// visited bits.
    fn clear_resident(&self, offset: StatePageOffset) {
        let (word, resident_mask, visited_mask) = offset.state_word();
        self.state[word].fetch_and(!(resident_mask | visited_mask), Ordering::Relaxed);
    }
}

/// Offset of one table slot within a [`StatePage`].
///
/// Values are in `0..STATE_PAGE_LEN`. Keeping the offset distinct from other
/// indexes prevents accidentally using a resident, pending-queue, or table-page
/// index to access the state-page bitmaps.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct StatePageOffset(usize);

impl StatePageOffset {
    const fn new(offset: usize) -> Self {
        debug_assert!(offset < STATE_PAGE_LEN);
        Self(offset)
    }

    const fn get(self) -> usize {
        self.0
    }

    /// Returns the word index, resident mask, and visited mask in
    /// [`StatePage::state`].
    const fn state_word(self) -> (usize, u64, u64) {
        let offset = self.get();
        let bit = offset % SLOTS_PER_STATE_WORD;
        let visited_mask = (1u32 << bit) as u64;
        let resident_mask = visited_mask << SLOTS_PER_STATE_WORD;
        (offset / SLOTS_PER_STATE_WORD, resident_mask, visited_mask)
    }

    /// Returns the word index and one-bit mask in [`PendingPage::pending`].
    const fn pending_word(self) -> (usize, u64) {
        let offset = self.get();
        let bit = offset % SLOTS_PER_PENDING_WORD;
        (offset / SLOTS_PER_PENDING_WORD, 1 << bit)
    }
}

/// Splits the table-slot index of an id into a state-page number and offset.
///
/// SIEVE state follows the physical table slot, so the generation component of
/// the id is intentionally not part of this mapping.
#[inline]
const fn state_page_number_and_offset(slot: u32) -> (StatePageNumber, StatePageOffset) {
    let index = slot as usize;
    let page_number = index >> STATE_PAGE_LEN_BITS;
    let offset = StatePageOffset::new(index & (STATE_PAGE_LEN - 1));
    let page_number = StatePageNumber::new(page_number)
        .expect("state-page directory should cover every page in a u32 id");
    (page_number, offset)
}

/// Reconstructs the physical table-slot id used by memo eviction.
///
/// Memo lookup uses only [`Id::index`]; the generation is irrelevant here.
const fn slot_id(page_number: StatePageNumber, offset: StatePageOffset) -> Id {
    let id = (page_number.get() << STATE_PAGE_LEN_BITS) | offset.get();
    // SAFETY: Callers recombine a page number and offset originally produced by
    // `state_page_number_and_offset` for an existing id.
    unsafe { Id::from_index(id as u32) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> Id {
        assert!(index < Id::MAX_U32);
        Id::from_bits(u64::from(index) + 1)
    }

    fn drain_evicted(sieve: &mut Sieve) -> Vec<Id> {
        let mut evicted = Vec::new();
        sieve.for_each_evicted(|id| evicted.push(id));
        evicted
    }

    #[test]
    fn evicting_the_newest_wraps_the_hand_before_admission() {
        let mut sieve = Sieve::new(2);
        sieve.record_use(id(0));
        sieve.record_use(id(0));
        sieve.record_use(id(1));
        sieve.record_use(id(2));
        assert_eq!(drain_evicted(&mut sieve), [id(1)]);

        // The hand wrapped to the survivor 0 before 2 joined the queue.
        sieve.record_use(id(3));
        assert_eq!(drain_evicted(&mut sieve), [id(0)]);
    }

    #[test]
    fn resizing_preserves_pending_evictions_and_survivors() {
        let mut sieve = Sieve::new(3);
        sieve.record_use(id(0));
        sieve.record_use(id(1));
        sieve.record_use(id(2));

        sieve.set_capacity(1);
        sieve.set_capacity(2);
        sieve.record_use(id(3));
        assert_eq!(drain_evicted(&mut sieve), [id(0), id(1)]);

        sieve.record_use(id(4));
        assert_eq!(drain_evicted(&mut sieve), [id(2)]);
    }

    #[test]
    fn disabling_discards_residents_and_ignores_uses() {
        let mut sieve = Sieve::new(1);
        let resident = id(0);
        let ignored_while_disabled = id(1);
        let incoming = id(2);

        sieve.record_use(resident);
        sieve.set_capacity(0);
        sieve.record_use(ignored_while_disabled);
        assert!(drain_evicted(&mut sieve).is_empty());

        sieve.set_capacity(1);
        sieve.record_use(resident);
        sieve.record_use(incoming);
        assert_eq!(drain_evicted(&mut sieve), [resident]);
    }

    #[test]
    fn drain_preserves_readmitted_slots_in_both_halves_of_a_pending_word() {
        let mut sieve = Sieve::new(2);
        sieve.record_use(id(30));
        sieve.record_use(id(31));
        sieve.record_use(id(32));
        sieve.record_use(id(63));
        sieve.record_use(id(64));

        // Both slots were selected and then re-admitted before the drain.
        sieve.record_use(id(31));
        sieve.record_use(id(32));
        assert_eq!(drain_evicted(&mut sieve), [id(30), id(63), id(64)]);
    }

    #[test]
    fn interrupted_sweep_keeps_remaining_page_indexes_valid() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let mut sieve = Sieve::new(1);
        let second = STATE_PAGE_LEN as u32;
        sieve.record_use(id(0));
        sieve.record_use(id(second));
        sieve.record_use(id(second + 1));
        sieve.record_use(id(second + 2));
        let result = catch_unwind(AssertUnwindSafe(|| {
            sieve.for_each_evicted(|_| panic!("interrupted sweep"));
        }));
        assert!(result.is_err());

        // Queue a fresh victim on the popped page. The first page is still pending.
        sieve.record_use(id(second + 3));
        let mut evicted = drain_evicted(&mut sieve);
        evicted.sort();
        assert_eq!(evicted, [id(0), id(second + 2)]);
    }

    #[test]
    fn state_page_number_covers_largest_id() {
        let id = id(Id::MAX_U32 - 1);
        let (page_number, offset) = state_page_number_and_offset(id.index());

        assert_eq!(slot_id(page_number, offset), id);
    }
}
