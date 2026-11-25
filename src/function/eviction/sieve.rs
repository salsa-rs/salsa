//! An implementation of the SIEVE cache eviction algorithm described in the
//! [NSDI '24 paper]. The [SIEVE project website] provides a visual explanation.
//!
//! # How SIEVE works
//!
//! SIEVE keeps residents in admission order, from newest to oldest, and keeps a
//! moving hand that starts at the oldest resident. Every resident also has one
//! `visited` bit:
//!
//! - New residents enter at the newest end with `visited = false`.
//! - A cache hit sets `visited = true`, but does not move the resident.
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
//! SIEVE therefore does not track exact recency. Its visited bit only answers
//! whether a resident was used since the hand last gave it a chance. This is
//! enough to protect reused values from one-shot scans while making a hit much
//! cheaper than moving a node in an LRU list.
//!
//! ## Bounded selection
//!
//! The textbook scan has no bound when concurrent hits can continually re-mark
//! entries while the hand scans. In Salsa, that could keep granting second
//! chances forever while holding the admission mutex.
//!
//! Victim selection therefore performs at most two inspections per current
//! resident. With no concurrent re-marks this is equivalent to textbook SIEVE:
//! one pass can clear every visited bit and the next inspection finds an
//! unvisited victim. The second pass also honors entries re-marked during the
//! first pass. If every inspection still observes a visited entry, Salsa evicts
//! the resident currently at the hand to guarantee progress while preserving
//! the scan order.
//!
//! # Implementation
//!
//! The main design goal is to keep cache hits lock-free. Salsa therefore splits
//! SIEVE's state into two parts:
//!
//! - `state_pages` contains the atomic resident and visited bits used by cache
//!   hits. A hit accesses only this state and does not acquire a mutex.
//! - `state` contains two deques in admission order and pending evictions.
//!   Admissions and victim selection access this state through a mutex.
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
//! └── state: Mutex<State>             admission and eviction state
//!     ├── passed: VecDeque<u32>       residents before the hand
//!     ├── candidates: VecDeque<u32>   residents from the hand to newest
//!     └── pending_pages: Vec<PendingPage>
//!         └── 1,024-bit bitmap        selected slots awaiting validation
//! ```
//!
//! The hit path looks up the [`StatePage`], atomically marks a resident as
//! visited, and returns. It falls back to [`State`] only when the id is not
//! resident or selection won a race with the hit. Supporting this lock-free
//! path drives the implementation complexity: ids map through a sparse,
//! atomically published page directory, state transitions must tolerate
//! races, and admissions and victim selection must coordinate the atomic state
//! with the mutex-protected deques.
//!
//! ## State-page mapping without a hash map
//!
//! A conventional standalone SIEVE cache stores the visited bit on each node in
//! the admission-order list and uses a hash map to find that node from a cache
//! key. Every hit therefore performs a hash-table lookup before it can set the
//! visited bit.
//!
//! Salsa can instead address replacement state from [`Id::index`], whose
//! consecutive values identify consecutive table slots. SIEVE partitions that
//! index into a state-page number and an offset within the state page:
//!
//! ```text
//! high bits   -> StatePageNumber
//! low 10 bits -> StatePageOffset
//! ```
//!
//! [`StatePages`] implements this mapping as a sparse geometric bucket
//! directory. The page number locates a [`StatePageCell`] without hashing, and
//! the cell lazily allocates one [`StatePage`] containing the bits selected by
//! the page offset. A hit can therefore find the resident and visited bits and
//! set `visited` without looking up the position of the slot in admission order.
//!
//! Allocating the corresponding memo-table pages does not by itself allocate
//! SIEVE state. A state page is allocated only when the enabled policy first
//! sees a use from its id range, and remains available for lock-free reads
//! until SIEVE is disabled or the policy is dropped.
//!
//! A state page deliberately covers 1,024 table slots, eight times the current
//! 128-slot memo-table page (as of July 5th 2026).
//! State pages and memo-table pages are independent:
//! memo slots contain comparatively large values, so smaller memo pages limit
//! unused table storage. SIEVE needs only two persistent bits per slot, making
//! a 1,024-slot resident/visited bitmap 256 bytes. The larger state page
//! amortizes the directory pointer, heap allocation, pending-queue index, and
//! pending-victim batching across more ids while keeping the excess bitmap
//! memory for a sparsely used page small.
//!
//! ## Deferred eviction
//!
//! Selecting a victim removes it from the queue and clears its resident bit
//! immediately, but Salsa cannot drop its memo value while queries may still
//! hold references from the current revision. The victim must remain pending
//! until the next policy drain, normally at a revision boundary.
//!
//! Pending victims are grouped using the same page-number-and-offset mapping. A
//! [`PendingPage`] stores one state-page number and a 1,024-bit bitmap; each set
//! bit identifies one selected offset from that page. A state page's
//! `pending_page_index` points back into `State::pending_pages`, so selecting
//! another victim from the same page finds the existing bitmap directly. Each
//! state page therefore appears at most once in the pending queue, without a
//! hash set or one vector entry per victim.
//!
//! During a drain, Salsa resolves each state page once and visits the set bits
//! in its pending bitmap. An access between selection and draining can re-admit
//! a victim and reuse its still-present memo, so the drain checks the resident
//! bit again. It deletes the memo only if the slot is still non-resident.
//!
//! ## Synchronization
//!
//! Admissions, hand movement, the resident deques, and the pending queue are
//! serialized by the state mutex. The resident/visited words deliberately sit
//! outside that protection so the common hit path does not acquire the mutex.
//! Victim selection holds the mutex, but that provides no mutual exclusion with
//! hits because they never acquire it. Both paths may access the same word at
//! the same time, so the words must be atomic.
//! The resident and visited bits for a slot are packed into the same atomic
//! word. This improves locality and lets the hit path observe both bits with a
//! single atomic load. It also prevents a race between checking residency and
//! setting `visited`: either the hit marks the still-resident slot and selection
//! observes that visit, or selection clears `resident` and the hit retries
//! through admission. Separate fields would require another atomic load and
//! permit a hit to observe `resident`, lose the race to selection, and then set
//! `visited` without re-admitting the selected slot.
//!
//! These atomics use relaxed ordering: the bits carry replacement-policy state
//! only and do not publish or protect memo data. In contrast, [`StatePageCell`]
//! uses acquire/release ordering because it publishes a newly allocated page
//! to concurrent readers.
//!
//! The per-slot state machine is:
//!
//! ```text
//! absent (00)                  --admit-->  resident, unvisited (01)
//! resident, unvisited (01)     --hit-->    resident, visited (11)
//! resident, visited (11)       --select--> resident, unvisited (01)
//! resident, unvisited (01)     --select--> absent (00) + pending eviction
//! absent + pending eviction    --readmit--> resident, unvisited (01)
//! ```
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

/// SIEVE eviction policy.
///
/// Values enter at the front of a FIFO queue. Uses atomically set a visited bit
/// in direct-addressed state pages; they do not move the value in the queue.
pub struct Sieve {
    /// Maximum number of residents. Zero disables SIEVE entirely.
    ///
    /// Mutation requires `&mut self`, so hit paths can read this without an
    /// atomic or the state mutex.
    capacity: usize,

    /// Lock-free lookup for the resident/visited bits used by cache hits.
    ///
    /// Allocated pages and their addresses remain valid until this directory
    /// is replaced while disabling SIEVE or the policy is dropped.
    state_pages: StatePages,

    /// Admission order, SIEVE hand, and pending-eviction pages.
    ///
    /// The mutex serializes admissions and all mutations to this state.
    state: Mutex<State>,
}

impl EvictionPolicy for Sieve {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state_pages: StatePages::new(),
            state: Mutex::new(State::with_capacity(capacity)),
        }
    }

    #[inline]
    fn record_use(&self, id: Id) {
        // Probe residency before capacity: resident hits do not need the capacity,
        // while admissions already take the cold path below.
        if let Some((page, offset)) = self.state_pages.try_page_and_offset(id)
            && page.record_use(offset)
        {
            return;
        }

        if self.capacity != 0 {
            self.record_admission(id);
        }
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

            pending_page.for_each_pending_offset(|offset| {
                // A victim may have been re-admitted since selection. Its stale
                // pending bit must not delete the memo it now reuses.
                if !page.is_resident(offset) {
                    evict(slot_id(pending_page.page_number, offset));
                }
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
    fn record_admission(&self, id: Id) {
        let (page, offset) = self.state_pages.page_and_offset_or_alloc(id);
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

/// Mutable replacement-policy state protected by [`Sieve::state`].
///
/// Invariants outside an in-progress mutation:
///
/// - In admission order (oldest first), the residents are `passed` followed
///   by `candidates`. The hand is the front of `candidates`.
/// - Every queued slot has its resident bit set in `StatePage::state`.
/// - Every `pending_pages[i]` is paired with a state page whose
///   `pending_page_index` stores `i + 1`, and no state page appears twice.
#[derive(Default)]
struct State {
    /// Slots given a second chance since the last wrap, oldest first.
    passed: VecDeque<u32>,
    /// Slots from the hand to the newest admission, oldest first.
    candidates: VecDeque<u32>,
    /// State pages containing victims whose memo values await deletion.
    ///
    /// Grouping victims by state page avoids one vector entry per victim and
    /// lets a drain reuse the resolved page for all of its pending slots.
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
        for queue in [&mut self.passed, &mut self.candidates] {
            queue.reserve_exact(capacity.saturating_sub(queue.len()));
        }
    }

    fn len(&self) -> usize {
        self.passed.len() + self.candidates.len()
    }

    fn schedule_eviction(&mut self, state_pages: &StatePages) {
        let mut budget = self.len().saturating_mul(2);
        loop {
            let slot = self
                .candidates
                .pop_front()
                .expect("non-empty resident queue");
            let (number, offset) = state_page_number_and_offset(slot);
            let page = &state_pages[number];

            let keep = if budget == 0 {
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

/// A transient batch of selected victims from one [`StatePage`].
///
/// `page_number` identifies both the state page and the high bits of every
/// pending id. `pending` stores the low page-offset bits. An id can be resident
/// and pending simultaneously after re-admission; the drain checks the
/// resident bitmap and skips that stale pending entry.
struct PendingPage {
    /// State-page number shared by every slot in `pending`.
    page_number: StatePageNumber,
    /// One bit per slot selected from this state page.
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

    /// Visits only set bits, in ascending page-offset order.
    ///
    /// This intentionally uses a callback instead of an iterator. An iterator
    /// must preserve the current word and word index across calls to `next`;
    /// measured iterator versions increased `fill_and_evict` instructions.
    fn for_each_pending_offset(&self, mut f: impl FnMut(StatePageOffset)) {
        for (word_index, &pending) in self.pending.iter().enumerate() {
            let mut pending = pending;
            while pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                f(StatePageOffset::new(
                    word_index * SLOTS_PER_PENDING_WORD + bit,
                ));
                pending &= pending - 1;
            }
        }
    }
}

/// Number of low slot-index bits used for the offset within a state page.
const STATE_PAGE_LEN_BITS: usize = 10;
/// Number of table slots represented by one independently allocated state page.
const STATE_PAGE_LEN: usize = 1 << STATE_PAGE_LEN_BITS;

/// Number of geometric buckets needed to cover every possible state-page number.
///
/// Boxcar skips its first small buckets, so [`buckets_for_index_bits`] may not
/// accept every index within the requested bit width. The additional bucket
/// covers the otherwise missing final indices.
const STATE_PAGE_BUCKETS: usize =
    buckets_for_index_bits(u32::BITS - STATE_PAGE_LEN_BITS as u32) + 1;

/// State-page number formed from the high bits of [`Id::index`].
type StatePageNumber = Index<STATE_PAGE_BUCKETS>;

/// Sparse, direct-addressed directory from [`StatePageNumber`] to
/// [`StatePageCell`].
///
/// The directory uses geometrically growing buckets. It allocates those
/// buckets and the [`StatePage`] owned by each cell lazily. Once published, an
/// allocation is neither moved nor freed until the directory is dropped, so
/// lock-free readers can safely retain references to it.
struct StatePages(Buckets<StatePageCell, STATE_PAGE_BUCKETS>);

impl StatePages {
    fn new() -> Self {
        Self(Buckets::new())
    }

    /// Returns the allocated state page and offset for `id` without allocating.
    ///
    /// Returns `None` if no state page has been allocated for the range
    /// containing `id`.
    #[inline]
    fn try_page_and_offset(&self, id: Id) -> Option<(&StatePage, StatePageOffset)> {
        let (page_number, offset) = state_page_number_and_offset(id.index());
        self.get(page_number).map(|page| (page, offset))
    }

    /// Returns the state page and offset for `id`, allocating the page if needed.
    ///
    /// Concurrent callers may race to allocate the same page; all receive the
    /// single allocation published by its [`StatePageCell`].
    ///
    /// # Panics
    ///
    /// May panic if allocating the directory bucket or state page fails.
    fn page_and_offset_or_alloc(&self, id: Id) -> (&StatePage, StatePageOffset) {
        let (page_number, offset) = state_page_number_and_offset(id.index());
        (self.0.get_or_alloc(page_number).get_or_alloc(), offset)
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

/// Lazily initialized owner of one [`StatePage`] allocation.
///
/// The surrounding [`StatePages`] directory allocates cells in buckets, but
/// each comparatively large state page is allocated only when an id in its
/// range first reaches the admission path. The pointer is installed once and
/// remains valid until the directory is exclusively dropped. A sparse, high id
/// can therefore reserve pointer slots for its directory bucket, but does not
/// allocate `StatePage`s for untouched entries in that bucket.
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
            // SAFETY: A non-null pointer was allocated by `get_or_alloc` and
            // remains owned by this slot until the directory is dropped.
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

/// Compact replacement-policy state for [`STATE_PAGE_LEN`] consecutive table slots.
///
/// Each slot has adjacent `resident` and `visited` bits. Keeping both bits in
/// one atomic word makes every transition indivisible with respect to a racing
/// hit: either the hit marks the still-resident value, or it observes that
/// selection removed the value and falls back to admission.
#[derive(Default)]
struct StatePage {
    /// Interleaved `[resident, visited]` bit pairs for all slots in the page.
    ///
    /// The only valid stable states are absent (`00`), resident/unvisited
    /// (`01`), and resident/visited (`11`).
    state: [AtomicU64; STATE_WORDS],
    /// One-based index into `State::pending_pages`, or zero when not queued.
    /// Mutation is serialized by the state mutex or exclusive access to the
    /// policy; the atomic preserves interior mutability without making pages
    /// unavailable to concurrent hit readers.
    pending_page_index: AtomicU32,
}

impl StatePage {
    /// Marks a resident as visited.
    ///
    /// Returns `true` if the slot was resident (including when it was already
    /// visited), allowing the hit path to return without locking. Returns
    /// `false` if selection won the race and the caller must attempt admission.
    #[inline]
    fn record_use(&self, offset: StatePageOffset) -> bool {
        let (word, resident_mask, visited_mask) = offset.state_word();
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            // Already marked visited. `visited` implies `resident`, so the hit
            // has been recorded.
            if state & visited_mask != 0 {
                return true;
            }

            // Not resident. The caller must return to the admission path.
            if state & resident_mask == 0 {
                return false;
            }

            // Mark the resident visited if the word still matches this
            // snapshot. Otherwise, recheck whether another hit marked it or
            // selection removed it.
            match word.compare_exchange_weak(
                state,
                state | visited_mask,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
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
            let clear = if visited { visited_mask } else { resident_mask };
            let new_state = state & !clear;
            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return visited,
                Err(new_state) => state = new_state,
            }
        }
    }

    /// Tests whether a pending victim has since been re-admitted.
    fn is_resident(&self, offset: StatePageOffset) -> bool {
        let (word, resident_mask, _) = offset.state_word();
        self.state[word].load(Ordering::Relaxed) & resident_mask != 0
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
        let bit = (offset % SLOTS_PER_STATE_WORD) * SLOT_STATE_BITS;
        let resident_mask = 1 << bit;
        let visited_mask = 1 << (bit + 1);
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
