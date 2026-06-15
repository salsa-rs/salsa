//! Adaptive generational eviction for memoized values.
//!
//! Values are tracked outside the memo in a timing wheel. Query fetches do not
//! touch the policy: reuse is detected lazily from the memo's existing
//! `verified_at` revision when a resident becomes due for inspection.

use std::collections::VecDeque;
use std::hash::BuildHasher;
use std::num::NonZeroUsize;

use crossbeam_utils::CachePadded;
use rustc_hash::FxBuildHasher;

use crate::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::{Mutex, OnceLock};
use crate::{Id, Revision};

use super::{EvictionPolicy, HasCapacity};

const GENERATIONS: u8 = 4;
const WHEEL_SIZE: usize = 64;
const GHOST_HORIZON: usize = 8;
const MIN_ADAPTIVE_SAMPLE: usize = 32;
const TARGET_BACKLOG_REVISIONS: usize = 8;

/// An adaptive generational collector selected by the existing `lru` option.
///
/// The configured value is a minimum per-revision maintenance budget, not a
/// hard resident-value limit.
pub struct Lru {
    maintenance_floor: Option<NonZeroUsize>,
    hasher: FxBuildHasher,
    shift: u32,
    admissions: Box<[CachePadded<Mutex<Vec<Admission>>>]>,
    state: State,
    current_epoch: AtomicUsize,
    ghosts: GhostSketch,
    refaults_by_epoch: [AtomicUsize; GHOST_HORIZON],
}

#[derive(Clone, Copy)]
struct Admission {
    id: Id,
    generation: u8,
}

struct Resident {
    id: Id,
    observed_at: Revision,
    generation: u8,
    cold_strikes: u8,
}

struct State {
    wheel: [Vec<Resident>; WHEEL_SIZE],
    /// Due buckets detached from the wheel so advancing the clock never has to
    /// process an entire cohort in one revision.
    due_cohorts: VecDeque<Vec<Resident>>,
    /// Empty cohort allocations retained for reuse by wheel buckets.
    spare_buffers: Vec<Vec<Resident>>,
    due_count: usize,
    epoch: usize,
    resident_count: usize,
    next_admission_shard: usize,
    evictions_by_epoch: [usize; GHOST_HORIZON],
    controller: AdaptiveController,
}

impl State {
    fn new() -> Self {
        Self {
            wheel: std::array::from_fn(|_| Vec::new()),
            due_cohorts: VecDeque::new(),
            spare_buffers: Vec::new(),
            due_count: 0,
            epoch: 0,
            resident_count: 0,
            next_admission_shard: 0,
            evictions_by_epoch: [0; GHOST_HORIZON],
            controller: AdaptiveController::default(),
        }
    }

    fn schedule(&mut self, resident: Resident, delay: usize) {
        debug_assert!(delay < WHEEL_SIZE);
        let bucket = self.epoch.wrapping_add(delay) & (WHEEL_SIZE - 1);
        if self.wheel[bucket].capacity() == 0 {
            if let Some(buffer) = self.spare_buffers.pop() {
                self.wheel[bucket] = buffer;
            }
        }
        self.wheel[bucket].push(resident);
    }

    fn queue_due_bucket(&mut self) {
        let bucket = self.epoch & (WHEEL_SIZE - 1);
        if self.wheel[bucket].is_empty() {
            return;
        }

        let due = std::mem::take(&mut self.wheel[bucket]);
        self.due_count += due.len();
        self.due_cohorts.push_back(due);
    }

    fn pop_due(&mut self) -> Option<Resident> {
        loop {
            if let Some(resident) = self.due_cohorts.front_mut()?.pop() {
                self.due_count -= 1;

                if self.due_cohorts.front().is_some_and(Vec::is_empty) {
                    self.recycle_front_cohort();
                }

                return Some(resident);
            }

            self.recycle_front_cohort();
        }
    }

    fn recycle_front_cohort(&mut self) {
        if let Some(buffer) = self.due_cohorts.pop_front() {
            if self.spare_buffers.len() < WHEEL_SIZE {
                self.spare_buffers.push(buffer);
            }
        }
    }

    fn maintenance_budget(&self, maintenance_floor: usize) -> usize {
        // At this rate, a burst decays geometrically while a continuous stream
        // reaches a stable backlog instead of growing without bound.
        maintenance_floor.max(self.due_count.div_ceil(TARGET_BACKLOG_REVISIONS))
    }
}

#[derive(Default)]
struct AdaptiveController {
    nursery_grace: u8,
    cold_strikes: u8,
    sampled_evictions: usize,
    sampled_refaults: usize,
}

impl AdaptiveController {
    fn initialize(&mut self) {
        if self.nursery_grace == 0 {
            self.nursery_grace = 2;
            self.cold_strikes = 2;
        }
    }

    fn observe(&mut self, evictions: usize, refaults: usize) {
        self.initialize();
        self.sampled_evictions = self.sampled_evictions.saturating_add(evictions);
        self.sampled_refaults = self.sampled_refaults.saturating_add(refaults);
        if self.sampled_evictions < MIN_ADAPTIVE_SAMPLE {
            return;
        }

        if self.sampled_refaults.saturating_mul(10) > self.sampled_evictions {
            self.nursery_grace = self.nursery_grace.saturating_add(1).min(8);
            self.cold_strikes = self.cold_strikes.saturating_add(1).min(4);
        } else if self.sampled_refaults.saturating_mul(100) < self.sampled_evictions {
            self.nursery_grace = self.nursery_grace.saturating_sub(1).max(2);
            self.cold_strikes = self.cold_strikes.saturating_sub(1).max(2);
        }

        self.sampled_evictions = 0;
        self.sampled_refaults = 0;
    }

    fn inspection_delay(&mut self, generation: u8) -> usize {
        self.initialize();
        if generation == 0 {
            usize::from(self.nursery_grace)
        } else {
            usize::from(self.nursery_grace) + (1usize << generation)
        }
    }
}

impl Lru {
    fn with_shards(maintenance_floor: usize, shards: usize) -> Self {
        assert!(shards > 1 && shards.is_power_of_two());

        Self {
            maintenance_floor: NonZeroUsize::new(maintenance_floor),
            hasher: FxBuildHasher,
            shift: usize::BITS - shards.trailing_zeros(),
            admissions: (0..shards).map(|_| Default::default()).collect(),
            state: State::new(),
            current_epoch: AtomicUsize::new(0),
            ghosts: GhostSketch::new(maintenance_floor),
            refaults_by_epoch: std::array::from_fn(|_| AtomicUsize::new(0)),
        }
    }

    #[inline]
    fn shard(&self, id: Id) -> usize {
        let hash = self.hasher.hash_one(id);
        ((hash as usize) << 7) >> self.shift
    }

    fn drain_admissions(&mut self, last_verified_at: &mut impl FnMut(Id) -> Option<Revision>) {
        let admissions: Vec<Vec<Admission>> = self
            .admissions
            .iter_mut()
            .map(|shard| std::mem::take(shard.get_mut()))
            .collect();
        let mut cursors = vec![0; admissions.len()];
        let mut remaining = admissions.iter().map(Vec::len).sum::<usize>();

        while remaining > 0 {
            for offset in 0..admissions.len() {
                let shard = (self.state.next_admission_shard + offset) & (admissions.len() - 1);
                let Some(&admission) = admissions[shard].get(cursors[shard]) else {
                    continue;
                };

                cursors[shard] += 1;
                remaining -= 1;
                let Some(observed_at) = last_verified_at(admission.id) else {
                    continue;
                };

                let delay = self.state.controller.inspection_delay(admission.generation);
                self.state.schedule(
                    Resident {
                        id: admission.id,
                        observed_at,
                        generation: admission.generation,
                        cold_strikes: 0,
                    },
                    delay,
                );
                self.state.resident_count += 1;
            }
            self.state.next_admission_shard =
                (self.state.next_admission_shard + 1) & (admissions.len() - 1);
        }

        for (shard, mut admissions) in self.admissions.iter_mut().zip(admissions) {
            admissions.clear();
            *shard.get_mut() = admissions;
        }
    }

    fn advance_epoch(
        &mut self,
        maintenance_floor: usize,
        last_verified_at: &mut impl FnMut(Id) -> Option<Revision>,
        evicted: &mut Vec<Id>,
    ) {
        self.state.epoch = self.state.epoch.wrapping_add(1);
        self.current_epoch
            .store(self.state.epoch, Ordering::Relaxed);

        if self.state.epoch >= GHOST_HORIZON {
            let expired_epoch = self.state.epoch - GHOST_HORIZON;
            let slot = expired_epoch % GHOST_HORIZON;
            let refaults = self.refaults_by_epoch[slot].swap(0, Ordering::Relaxed);
            let evictions = std::mem::take(&mut self.state.evictions_by_epoch[slot]);
            self.state.controller.observe(evictions, refaults);
        }

        self.state.queue_due_bucket();
        let budget = self.state.maintenance_budget(maintenance_floor);
        for _ in 0..budget {
            let Some(mut resident) = self.state.pop_due() else {
                break;
            };
            let Some(verified_at) = last_verified_at(resident.id) else {
                self.state.resident_count = self.state.resident_count.saturating_sub(1);
                continue;
            };

            if verified_at > resident.observed_at {
                resident.observed_at = verified_at;
                resident.generation = resident.generation.saturating_add(1).min(GENERATIONS - 1);
                resident.cold_strikes = 0;
                let delay = self.state.controller.inspection_delay(resident.generation);
                self.state.schedule(resident, delay);
                continue;
            }

            if resident.generation > 0 {
                resident.generation -= 1;
                resident.cold_strikes = 0;
                let delay = self.state.controller.inspection_delay(resident.generation);
                self.state.schedule(resident, delay);
                continue;
            }

            resident.cold_strikes += 1;
            self.state.controller.initialize();
            if resident.cold_strikes < self.state.controller.cold_strikes {
                self.state.schedule(resident, 1);
                continue;
            }

            self.ghosts
                .record(resident.id, self.state.epoch, resident.generation);
            self.state.evictions_by_epoch[self.state.epoch % GHOST_HORIZON] += 1;
            self.state.resident_count = self.state.resident_count.saturating_sub(1);
            evicted.push(resident.id);
        }
    }
}

impl EvictionPolicy for Lru {
    fn new(maintenance_floor: usize) -> Self {
        static SHARDS: OnceLock<usize> = OnceLock::new();
        let shards = *SHARDS.get_or_init(|| {
            let parallelism = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);
            (parallelism * 4).next_power_of_two()
        });

        Self::with_shards(maintenance_floor, shards)
    }

    #[inline(always)]
    fn admit(&self, id: Id) {
        if self.maintenance_floor.is_none() {
            return;
        }

        let epoch = self.current_epoch.load(Ordering::Relaxed);
        let generation = self
            .ghosts
            .refault(id, epoch)
            .map(|(generation, eviction_epoch)| {
                self.refaults_by_epoch[eviction_epoch % GHOST_HORIZON]
                    .fetch_add(1, Ordering::Relaxed);
                generation.saturating_add(1).min(GENERATIONS - 1)
            })
            .unwrap_or(0);
        self.admissions[self.shard(id)]
            .lock()
            .push(Admission { id, generation });
    }

    #[inline(always)]
    fn promote(&self, _id: Id) {}

    fn set_capacity(&mut self, maintenance_floor: usize) {
        self.maintenance_floor = NonZeroUsize::new(maintenance_floor);
        if self.maintenance_floor.is_none() {
            for admissions in &mut self.admissions {
                admissions.get_mut().clear();
            }
            self.state = State::new();
            self.current_epoch.store(0, Ordering::Relaxed);
            self.ghosts.clear();
            for refaults in &mut self.refaults_by_epoch {
                *refaults.get_mut() = 0;
            }
        }
    }

    fn evict(&mut self, mut last_verified_at: impl FnMut(Id) -> Option<Revision>) -> Vec<Id> {
        let Some(maintenance_floor) = self.maintenance_floor.map(NonZeroUsize::get) else {
            return Vec::new();
        };

        self.drain_admissions(&mut last_verified_at);
        if self.state.resident_count == 0 {
            return Vec::new();
        }

        let mut evicted = Vec::new();
        self.advance_epoch(maintenance_floor, &mut last_verified_at, &mut evicted);
        evicted
    }
}

impl HasCapacity for Lru {}

struct GhostSketch {
    slots: Box<[AtomicU64]>,
}

impl GhostSketch {
    const VALID: u64 = 1 << 63;
    const EPOCH_MASK: usize = (1 << 24) - 1;

    fn new(maintenance_floor: usize) -> Self {
        let len = maintenance_floor
            .saturating_mul(4)
            .clamp(256, 65_536)
            .next_power_of_two();
        Self {
            slots: (0..len).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot.get_mut() = 0;
        }
    }

    fn record(&self, id: Id, epoch: usize, generation: u8) {
        let hash = hash(id);
        let fingerprint = (hash >> 32) as u32 | 1;
        let packed = Self::VALID
            | u64::from(fingerprint)
            | (((epoch & Self::EPOCH_MASK) as u64) << 32)
            | (u64::from(generation) << 56);
        self.slots[hash as usize & (self.slots.len() - 1)].store(packed, Ordering::Relaxed);
    }

    fn refault(&self, id: Id, current_epoch: usize) -> Option<(u8, usize)> {
        let hash = hash(id);
        let packed = self.slots[hash as usize & (self.slots.len() - 1)].load(Ordering::Relaxed);
        let fingerprint = (hash >> 32) as u32 | 1;
        if packed & Self::VALID == 0 || packed as u32 != fingerprint {
            return None;
        }

        let eviction_epoch = ((packed >> 32) as usize) & Self::EPOCH_MASK;
        let age =
            (current_epoch & Self::EPOCH_MASK).wrapping_sub(eviction_epoch) & Self::EPOCH_MASK;
        if age >= GHOST_HORIZON {
            return None;
        }

        Some((((packed >> 56) & 0x7f) as u8, eviction_epoch))
    }
}

#[inline]
fn hash(id: Id) -> u64 {
    let mut value = id.as_bits();
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;

    fn id(index: u32) -> Id {
        // SAFETY: Test indices are all below `Id::MAX_U32`.
        unsafe { Id::from_index(index) }
    }

    fn revisions(entries: &[(Id, Revision)]) -> impl FnMut(Id) -> Option<Revision> + '_ {
        |id| {
            entries
                .iter()
                .find_map(|&(candidate, revision)| (candidate == id).then_some(revision))
        }
    }

    #[test]
    fn cold_resident_ages_without_new_admissions() {
        let mut lru = Lru::with_shards(1, 2);
        let cold = id(0);
        let revision = Revision::start();

        lru.admit(cold);
        assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        assert_eq!(lru.evict(revisions(&[(cold, revision)])), [cold]);
    }

    #[test]
    fn reuse_promotes_a_resident() {
        let mut lru = Lru::with_shards(1, 2);
        let reused = id(0);

        lru.admit(reused);
        assert_eq!(lru.evict(revisions(&[(reused, Revision::start())])), []);
        assert_eq!(lru.evict(revisions(&[(reused, Revision::from(2))])), []);
        for _ in 0..4 {
            assert_eq!(lru.evict(revisions(&[(reused, Revision::from(2))])), []);
        }
    }

    #[test]
    fn large_due_cohort_is_processed_incrementally() {
        let mut lru = Lru::with_shards(1, 2);
        let entries: Vec<_> = (0..32)
            .map(|index| (id(index), Revision::start()))
            .collect();

        for &(id, _) in &entries {
            lru.admit(id);
        }

        assert_eq!(lru.evict(revisions(&entries)), []);
        assert_eq!(lru.state.due_count, 0);

        assert_eq!(lru.evict(revisions(&entries)), []);
        assert_eq!(lru.state.due_count, 28);
        assert_eq!(lru.state.resident_count, 32);
    }

    #[test]
    fn continuous_cold_admissions_converge() {
        const ADMISSIONS_PER_REVISION: u32 = 32;

        let mut lru = Lru::with_shards(1, 2);
        for revision in 0..256 {
            for offset in 0..ADMISSIONS_PER_REVISION {
                lru.admit(id(revision * ADMISSIONS_PER_REVISION + offset));
            }
            lru.evict(|_| Some(Revision::start()));
        }

        assert!(
            lru.state.resident_count < ADMISSIONS_PER_REVISION as usize * 32,
            "cold resident set did not converge: {}",
            lru.state.resident_count
        );
    }

    #[test]
    fn refault_feedback_increases_retention() {
        let mut controller = AdaptiveController::default();
        controller.observe(100, 20);

        assert_eq!(controller.nursery_grace, 3);
        assert_eq!(controller.cold_strikes, 3);
    }

    #[test]
    fn ghost_sketch_recognizes_recent_eviction() {
        let ghosts = GhostSketch::new(1);
        let id = id(0);
        ghosts.record(id, 4, 2);

        assert_eq!(ghosts.refault(id, 5), Some((2, 4)));
        assert_eq!(ghosts.refault(id, 12), None);
    }

    #[test]
    fn hashes_strided_ids_across_admission_shards() {
        let lru = Lru::with_shards(1, 64);
        let mut shard_counts = [0; 64];

        for index in (0..64 * 4096).step_by(64) {
            shard_counts[lru.shard(id(index))] += 1;
        }

        let min = shard_counts.into_iter().min().unwrap();
        let max = shard_counts.into_iter().max().unwrap();
        assert!(max - min <= 8, "{shard_counts:?}");
    }

    #[test]
    fn preserves_admission_capacity_between_revisions() {
        let mut lru = Lru::with_shards(1, 2);
        let candidate = id(0);
        let shard = lru.shard(candidate);
        let revision = Revision::start();

        lru.admit(candidate);
        let capacity = lru.admissions[shard].get_mut().capacity();
        assert!(capacity > 0);

        assert_eq!(lru.evict(revisions(&[(candidate, revision)])), []);
        assert_eq!(lru.admissions[shard].get_mut().capacity(), capacity);
    }
}
