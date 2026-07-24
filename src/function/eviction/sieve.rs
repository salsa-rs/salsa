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
//! - `state` contains the admission-order list, the hand, and pending evictions.
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
//!     ├── residents: Residents        intrusive admission-order list
//!     ├── hand: Option<ResidentIndex> next candidate in that list
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
//! with the mutex-protected list.
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
//! set `visited` without finding the corresponding [`Resident`] node; those
//! nodes are used only while admitting or selecting residents under the state
//! mutex.
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
//! Selecting a victim removes it from [`Residents`] and clears its resident bit
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
//! Admissions, hand movement, the resident list, and the pending queue are
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

    /// Admission-order list, SIEVE hand, and pending-eviction pages.
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
        if let Some((page, offset)) = self.state_pages.try_page_and_offset(id) {
            if page.record_use(offset) {
                return;
            }
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
        for (queue_index, pending_page) in state.pending_pages.drain(..).enumerate() {
            let page = &self.state_pages[pending_page.page_number];

            // Queue indexes refer to positions before `drain`. Enumerating in
            // order lets each page validate and clear its reverse mapping.
            page.clear_pending_page_index(queue_index);

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
            state.residents.reserve_capacity(capacity);
            state.schedule_excess_evictions(capacity, &self.state_pages);
        }
    }
}

impl Sieve {
    fn record_admission(&self, id: Id) {
        let (page, offset) = self.state_pages.page_and_offset_or_alloc(id);
        let mut state = self.state.lock();

        if page.is_resident(offset) {
            page.record_use(offset);
            return;
        }

        state.insert(id, page, offset, self.capacity, &self.state_pages);
    }
}

/// Mutable replacement-policy state protected by [`Sieve::state`].
///
/// Invariants outside an in-progress mutation:
///
/// - `hand` is `None` exactly when `residents` is empty; otherwise it names a
///   resident node.
/// - Every list node has its resident bit set in `StatePage::state`.
/// - Every `pending_pages[i]` is paired with a state page whose
///   `pending_page_index` stores `i + 1`, and no state page appears twice.
#[derive(Default)]
struct State {
    /// Residents in admission order, newest at the front and oldest at the back.
    residents: Residents,
    /// Next resident candidate to inspect, or `None` when the list is empty.
    hand: Option<ResidentIndex>,
    /// State pages containing victims whose memo values await deletion.
    ///
    /// Grouping victims by state page avoids one vector entry per victim and
    /// lets a drain reuse the resolved page for all of its pending slots.
    pending_pages: Vec<PendingPage>,
}

impl State {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            residents: Residents::with_capacity(capacity),
            hand: None,
            pending_pages: Vec::new(),
        }
    }

    fn insert(
        &mut self,
        id: Id,
        page: &StatePage,
        offset: StatePageOffset,
        capacity: usize,
        state_pages: &StatePages,
    ) {
        debug_assert!(self.residents.len() <= capacity);
        if self.residents.len() == capacity {
            self.schedule_eviction(state_pages);
        }

        let node = self.residents.push_front(id);
        page.admit(offset);

        // The first resident starts the scan. Later admissions join at the
        // front without disturbing the hand's current position.
        if self.hand.is_none() {
            self.hand = Some(node);
        }
    }

    /// Selects residents above `capacity` for deferred eviction.
    fn schedule_excess_evictions(&mut self, capacity: usize, state_pages: &StatePages) {
        while self.residents.len() > capacity {
            self.schedule_eviction(state_pages);
        }
    }

    fn schedule_eviction(&mut self, state_pages: &StatePages) {
        let victim = self.select_victim(state_pages);
        self.mark_pending(victim);
    }

    /// Adds a victim to its page batch, queuing that page at most once.
    ///
    /// `StatePage::pending_page_index` is the reverse mapping that turns page
    /// deduplication into a direct lookup instead of a queue scan.
    fn mark_pending(&mut self, victim: SelectedVictim<'_>) {
        let queue_index = victim.page.pending_page_index().unwrap_or_else(|| {
            let page_number = state_page_number_and_offset(victim.id).0;
            let queue_index = self.pending_pages.len();
            self.pending_pages.push(PendingPage::new(page_number));
            victim.page.set_pending_page_index(queue_index);
            queue_index
        });

        let pending_page = &mut self.pending_pages[queue_index];
        debug_assert_eq!(
            pending_page.page_number,
            state_page_number_and_offset(victim.id).0
        );
        pending_page.mark(victim.offset);
    }

    /// Selects and removes the next resident using a bounded SIEVE scan.
    ///
    /// The scan performs at most two inspections per current resident. The
    /// second pass preserves second chances for concurrent re-marks; exhausting
    /// both passes force-evicts at the hand to guarantee progress.
    ///
    /// # Panics
    ///
    /// Panics if `self.hand` is `None`, which indicates that the resident list
    /// is empty.
    fn select_victim<'a>(&mut self, state_pages: &'a StatePages) -> SelectedVictim<'a> {
        let mut hand = self
            .hand
            .expect("non-empty resident list should have a hand");

        let inspection_budget = self.residents.len().saturating_mul(2);
        for _ in 0..inspection_budget {
            let id = self.residents.id(hand);
            let (page, offset) = state_pages.page_and_offset(id);

            if page.select(offset).was_visited() {
                // Give a visited resident a second chance. Selection cleared
                // its visited bit but left it resident, so inspect the next
                // newer candidate.
                hand = self.residents.wrapping_prev(hand);
            } else {
                // Selection made this unvisited candidate non-resident. Remove
                // it from admission order and return it for pending eviction.
                return self.remove_victim(hand, page, offset);
            }
        }

        // Every inspected resident was re-marked. Evict at the hand to
        // preserve SIEVE's scan order while guaranteeing progress.
        let id = self.residents.id(hand);
        let (page, offset) = state_pages.page_and_offset(id);
        page.clear_resident(offset);
        self.remove_victim(hand, page, offset)
    }

    fn remove_victim<'a>(
        &mut self,
        index: ResidentIndex,
        page: &'a StatePage,
        offset: StatePageOffset,
    ) -> SelectedVictim<'a> {
        let next_hand = if self.residents.len() == 1 {
            None
        } else {
            Some(self.residents.wrapping_prev(index))
        };
        let id = self.residents.remove(index);
        self.hand = next_hand;

        SelectedVictim { id, page, offset }
    }
}

/// A victim removed from [`Residents`] and atomically marked non-resident.
///
/// The page reference and offset are carried out of selection so queuing the
/// victim does not repeat the sparse-directory lookup.
struct SelectedVictim<'a> {
    /// Full id retained by the resident list.
    id: Id,

    /// The already-resolved location avoids another state-directory lookup when the
    /// victim is added to the pending-eviction queue.
    page: &'a StatePage,

    /// Offset of `id.index()` within `page`.
    offset: StatePageOffset,
}

/// Number of ids represented by one pending-eviction word.
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

/// Index into [`Residents::nodes`]; zero is always the sentinel.
type ResidentIndex = u32;

/// Intrusive circular list that owns the full ids of current residents.
///
/// `nodes[0]` is a permanent sentinel. Its `next` points to the newest
/// resident (the front) and its `prev` points to the oldest (the back). For a
/// resident node, `prev` points toward newer entries and `next` toward older
/// entries. SIEVE's hand walks through `prev`, wrapping from the newest entry
/// back to the oldest.
///
/// Removing a node clears its id and links it into an intrusive free list via
/// `next`. This avoids a second allocation-prone vector and lets later
/// admissions reuse storage. Consequently `nodes` grows to the peak number of
/// simultaneous residents plus the sentinel, rather than with total admissions.
/// Its backing allocation is eagerly reserved for the configured capacity and
/// retained across nonzero capacity changes.
///
/// There is deliberately no map from id to resident node. Hits never move list
/// nodes, and victim selection already has the node index stored in the hand;
/// direct id lookup is needed only for the bits in [`StatePage`].
struct Residents {
    nodes: Vec<Resident>,
    /// Intrusive free list linked through `Resident::next`.
    free_list_head: ResidentIndex,
    len: usize,
}

impl Default for Residents {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl Residents {
    /// Permanent list sentinel and empty free-list marker.
    const SENTINEL: ResidentIndex = 0;

    fn with_capacity(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(Self::capacity_with_sentinel(capacity));
        nodes.push(Resident {
            id: None,
            prev: Self::SENTINEL,
            next: Self::SENTINEL,
        });

        Self {
            nodes,
            free_list_head: Self::SENTINEL,
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn push_front(&mut self, id: Id) -> ResidentIndex {
        let index = self.alloc_node(id);
        let first = self[Self::SENTINEL].next;

        let node = &mut self[index];
        node.prev = Self::SENTINEL;
        node.next = first;
        self[Self::SENTINEL].next = index;
        self[first].prev = index;

        self.len += 1;
        index
    }

    fn remove(&mut self, index: ResidentIndex) -> Id {
        debug_assert_ne!(index, Self::SENTINEL);

        let free_list_head = self.free_list_head;
        let node = &mut self[index];
        let prev = node.prev;
        let next = node.next;
        let id = node.id.take().expect("resident node should have an id");
        node.prev = Self::SENTINEL;
        node.next = free_list_head;

        self[prev].next = next;
        self[next].prev = prev;
        self.len -= 1;
        self.free_list_head = index;
        id
    }

    fn id(&self, index: ResidentIndex) -> Id {
        self[index].id.expect("resident node should have an id")
    }

    /// Returns `index.prev`, wrapping past the sentinel to the list's tail.
    fn wrapping_prev(&self, index: ResidentIndex) -> ResidentIndex {
        debug_assert_ne!(index, Self::SENTINEL);

        let prev = self[index].prev;
        if prev == Self::SENTINEL {
            self[Self::SENTINEL].prev
        } else {
            prev
        }
    }

    fn alloc_node(&mut self, id: Id) -> ResidentIndex {
        // Reuse a tombstoned node from the intrusive free list.
        if self.free_list_head != Self::SENTINEL {
            let index = self.free_list_head;
            let node = &mut self[index];
            let next = node.next;
            node.id = Some(id);
            self.free_list_head = next;
            index
        } else {
            // Append storage for a new resident node.
            let index = ResidentIndex::try_from(self.nodes.len())
                .expect("SIEVE resident capacity should fit in u32");
            self.nodes.push(Resident {
                id: Some(id),
                prev: Self::SENTINEL,
                next: Self::SENTINEL,
            });
            index
        }
    }

    fn reserve_capacity(&mut self, capacity: usize) {
        let node_capacity = Self::capacity_with_sentinel(capacity);
        if self.nodes.capacity() < node_capacity {
            self.nodes.reserve_exact(node_capacity - self.nodes.len());
        }
    }

    fn capacity_with_sentinel(resident_capacity: usize) -> usize {
        resident_capacity
            .checked_add(1)
            .expect("SIEVE resident capacity should leave room for the sentinel")
    }
}

impl std::ops::Index<ResidentIndex> for Residents {
    type Output = Resident;

    fn index(&self, index: ResidentIndex) -> &Self::Output {
        &self.nodes[index as usize]
    }
}

impl std::ops::IndexMut<ResidentIndex> for Residents {
    fn index_mut(&mut self, index: ResidentIndex) -> &mut Self::Output {
        &mut self.nodes[index as usize]
    }
}

/// One allocated slot in the resident list or its free list.
struct Resident {
    /// Full id while resident; `None` for the sentinel and free nodes.
    id: Option<Id>,
    /// Newer resident, or the sentinel.
    prev: ResidentIndex,
    /// Older resident, or the sentinel.
    next: ResidentIndex,
}

/// Number of low id-index bits used as the offset within a state page.
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
        let (page_number, offset) = state_page_number_and_offset(id);
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
        let (page_number, offset) = state_page_number_and_offset(id);
        (self.0.get_or_alloc(page_number).get_or_alloc(), offset)
    }

    /// Returns the already-allocated state page and offset for `id`.
    ///
    /// # Panics
    ///
    /// Panics if no state page has been allocated for the range containing
    /// `id`. Resident and pending ids always satisfy this requirement.
    fn page_and_offset(&self, id: Id) -> (&StatePage, StatePageOffset) {
        let (page_number, offset) = state_page_number_and_offset(id);
        (&self[page_number], offset)
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
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            let new_state = (state | resident_mask) & !visited_mask;
            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(new_state) => state = new_state,
            }
        }
    }

    /// Atomically grants a second chance or makes an unvisited slot
    /// non-resident and eligible for pending eviction.
    fn select(&self, offset: StatePageOffset) -> Selection {
        let (word, resident_mask, visited_mask) = offset.state_word();
        let word = &self.state[word];
        let mut state = word.load(Ordering::Relaxed);

        loop {
            debug_assert_ne!(
                state & resident_mask,
                0,
                "resident list entry should have a resident state bit"
            );

            let (new_state, selection) = if state & visited_mask == 0 {
                // Unvisited: remove it from the resident set and evict it.
                (state & !(resident_mask | visited_mask), Selection::Evict)
            } else {
                // Visited: keep it resident, clear the mark, and grant a
                // second chance.
                (state & !visited_mask, Selection::SecondChance)
            };

            match word.compare_exchange_weak(state, new_state, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return selection,
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

/// Result of inspecting one resident at the SIEVE hand.
#[derive(Copy, Clone)]
enum Selection {
    SecondChance,
    Evict,
}

impl Selection {
    const fn was_visited(self) -> bool {
        matches!(self, Selection::SecondChance)
    }
}

/// Splits the table-slot index of an id into a state-page number and offset.
///
/// SIEVE state follows the physical table slot, so the generation component of
/// the id is intentionally not part of this mapping.
#[inline]
const fn state_page_number_and_offset(id: Id) -> (StatePageNumber, StatePageOffset) {
    let index = id.index() as usize;
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
    fn all_marked_residents_evict_the_initial_hand() {
        let mut sieve = Sieve::new(3);
        let oldest = id(0);
        let middle = id(1);
        let newest = id(2);

        for id in [oldest, middle, newest] {
            // Admission starts unvisited; the second use marks the resident.
            sieve.record_use(id);
            sieve.record_use(id);
        }

        sieve.set_capacity(2);
        assert_eq!(drain_evicted(&mut sieve), [oldest]);

        // The hand resumes with the resident after the first victim.
        sieve.set_capacity(1);
        assert_eq!(drain_evicted(&mut sieve), [middle]);
    }

    #[test]
    fn insertion_selects_a_victim_before_admitting() {
        let mut sieve = Sieve::new(2);
        let oldest = id(0);
        let newest = id(1);
        let incoming = id(2);

        for id in [oldest, newest] {
            // Ensure the existing residents receive a second chance while the
            // incoming, initially unvisited resident does not participate.
            sieve.record_use(id);
            sieve.record_use(id);
        }

        sieve.record_use(incoming);
        assert_eq!(drain_evicted(&mut sieve), [oldest]);
    }

    #[test]
    fn capacity_changes_preserve_policy_state_and_evict_all_excess() {
        let mut sieve = Sieve::new(5);
        for index in 0..5 {
            sieve.record_use(id(index));
        }

        sieve.set_capacity(2);
        sieve.set_capacity(8);
        assert_eq!(drain_evicted(&mut sieve), [id(0), id(1), id(2)]);

        for index in 5..11 {
            sieve.record_use(id(index));
        }
        assert!(drain_evicted(&mut sieve).is_empty());

        sieve.record_use(id(11));
        assert_eq!(drain_evicted(&mut sieve), [id(3)]);
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
    fn repeated_readmission_reports_each_nonresident_once() {
        let mut sieve = Sieve::new(1);
        let first = id(0);
        let second = id(1);

        for _ in 0..2 {
            sieve.record_use(first);
            sieve.record_use(second);
        }

        assert_eq!(drain_evicted(&mut sieve), [first]);

        sieve.record_use(first);
        assert_eq!(drain_evicted(&mut sieve), [second]);
    }

    #[test]
    fn evictions_across_state_pages_are_all_reported() {
        let mut sieve = Sieve::new(1);
        let first = id(0);
        let second = id(STATE_PAGE_LEN as u32);
        let third = id(1);
        let resident = id(STATE_PAGE_LEN as u32 + 1);

        for id in [first, second, third, resident] {
            sieve.record_use(id);
        }

        let mut evicted = drain_evicted(&mut sieve);
        evicted.sort();
        assert_eq!(evicted, [first, third, second]);
    }

    #[test]
    fn state_page_number_covers_largest_id() {
        let id = id(Id::MAX_U32 - 1);
        let (page_number, offset) = state_page_number_and_offset(id);

        assert_eq!(
            page_number.get(),
            id.index() as usize >> STATE_PAGE_LEN_BITS
        );
        assert_eq!(offset.get(), id.index() as usize & (STATE_PAGE_LEN - 1));
        assert_eq!(slot_id(page_number, offset), id);
    }
}
