//! Adaptive generational eviction for memoized values.
//!
//! Values are tracked outside the memo in timing wheels. Query fetches do not
//! touch the policy: reuse is detected lazily from the memo's existing
//! `verified_at` revision when a value becomes due for inspection.

use std::collections::VecDeque;
use std::hash::BuildHasher;
use std::num::NonZeroUsize;

use crossbeam_utils::CachePadded;
use rustc_hash::FxBuildHasher;

use crate::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::{Mutex, OnceLock};
use crate::{Id, Revision};

use super::{EvictionPolicy, HasCapacity};

const RESIDENT_GENERATIONS: u8 = 3;
const WHEEL_SIZE: usize = 64;
const PROBATION_DELAY: usize = 16;
const RESIDENT_BASE_DELAY: usize = 24;
const RESIDENT_DELAY_STEP: usize = 16;
const GHOST_HORIZON: usize = 64;
const FEEDBACK_HORIZON: usize = 8;
const MIN_ADAPTIVE_SAMPLE: usize = 32;
const TARGET_BACKLOG_REVISIONS: usize = 8;
const PROBATION_ADMISSION: u8 = u8::MAX;
const _: () = assert!(PROBATION_DELAY < WHEEL_SIZE);
const _: () = assert!(
    RESIDENT_BASE_DELAY + (RESIDENT_GENERATIONS as usize - 1) * RESIDENT_DELAY_STEP + 7
        < WHEEL_SIZE
);

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
    refaults_by_epoch: [AtomicUsize; FEEDBACK_HORIZON],
}

#[derive(Clone, Copy)]
struct Admission {
    id: Id,
    /// `PROBATION_ADMISSION` for first-time admissions; otherwise the resident
    /// generation to restore after a refault.
    generation: u8,
}

struct Probation {
    id: Id,
    observed_at: Revision,
}

struct Resident {
    id: Id,
    observed_at: Revision,
    generation: u8,
    cold_strikes: u8,
}

struct State {
    probation_cohorts: VecDeque<ProbationCohort>,
    resident_wheel: [Vec<Resident>; WHEEL_SIZE],
    /// Due buckets detached from the wheel so advancing the clock never has to
    /// process an entire cohort in one revision.
    due_cohorts: VecDeque<DueCohort>,
    /// Empty cohort allocations retained for reuse by wheel buckets.
    spare_probation_buffers: Vec<Vec<Probation>>,
    spare_resident_buffers: Vec<Vec<Resident>>,
    due_count: usize,
    epoch: usize,
    entry_count: usize,
    next_admission_shard: usize,
    evictions_by_epoch: [usize; FEEDBACK_HORIZON],
    controller: AdaptiveController,
}

struct ProbationCohort {
    due_epoch: usize,
    entries: Vec<Probation>,
}

enum DueCohort {
    Probation(Vec<Probation>),
    Resident(Vec<Resident>),
}

enum DueEntry {
    Probation(Probation),
    Resident(Resident),
}

impl State {
    fn new() -> Self {
        Self {
            probation_cohorts: VecDeque::new(),
            resident_wheel: std::array::from_fn(|_| Vec::new()),
            due_cohorts: VecDeque::new(),
            spare_probation_buffers: Vec::new(),
            spare_resident_buffers: Vec::new(),
            due_count: 0,
            epoch: 0,
            entry_count: 0,
            next_admission_shard: 0,
            evictions_by_epoch: [0; FEEDBACK_HORIZON],
            controller: AdaptiveController::default(),
        }
    }

    fn schedule_probation(&mut self, probation: Probation) {
        let due_epoch = self.epoch.wrapping_add(PROBATION_DELAY);
        if let Some(cohort) = self
            .probation_cohorts
            .back_mut()
            .filter(|cohort| cohort.due_epoch == due_epoch)
        {
            cohort.entries.push(probation);
            return;
        }

        let mut entries = self.spare_probation_buffers.pop().unwrap_or_default();
        entries.push(probation);
        self.probation_cohorts
            .push_back(ProbationCohort { due_epoch, entries });
    }

    fn schedule_resident(&mut self, resident: Resident, delay: usize) {
        debug_assert!(delay < WHEEL_SIZE);
        let bucket = self.epoch.wrapping_add(delay) & (WHEEL_SIZE - 1);
        if self.resident_wheel[bucket].capacity() == 0 {
            if let Some(buffer) = self.spare_resident_buffers.pop() {
                self.resident_wheel[bucket] = buffer;
            }
        }
        self.resident_wheel[bucket].push(resident);
    }

    fn queue_due_bucket(&mut self) {
        while self
            .probation_cohorts
            .front()
            .is_some_and(|cohort| cohort.due_epoch == self.epoch)
        {
            let cohort = self.probation_cohorts.pop_front().unwrap();
            self.due_count += cohort.entries.len();
            self.due_cohorts
                .push_back(DueCohort::Probation(cohort.entries));
        }

        let bucket = self.epoch & (WHEEL_SIZE - 1);
        if !self.resident_wheel[bucket].is_empty() {
            let due = std::mem::take(&mut self.resident_wheel[bucket]);
            self.due_count += due.len();
            self.due_cohorts.push_back(DueCohort::Resident(due));
        }
    }

    fn pop_due(&mut self) -> Option<DueEntry> {
        loop {
            let entry = match self.due_cohorts.front_mut()? {
                DueCohort::Probation(probations) => probations.pop().map(DueEntry::Probation),
                DueCohort::Resident(residents) => residents.pop().map(DueEntry::Resident),
            };

            if let Some(entry) = entry {
                self.due_count -= 1;
                if self.front_cohort_is_empty() {
                    self.recycle_front_cohort();
                }
                return Some(entry);
            }

            self.recycle_front_cohort();
        }
    }

    fn front_cohort_is_empty(&self) -> bool {
        self.due_cohorts.front().is_some_and(|cohort| match cohort {
            DueCohort::Probation(probations) => probations.is_empty(),
            DueCohort::Resident(residents) => residents.is_empty(),
        })
    }

    fn recycle_front_cohort(&mut self) {
        match self.due_cohorts.pop_front() {
            Some(DueCohort::Probation(buffer))
                if self.spare_probation_buffers.len() < WHEEL_SIZE =>
            {
                self.spare_probation_buffers.push(buffer);
            }
            Some(DueCohort::Resident(buffer)) if self.spare_resident_buffers.len() < WHEEL_SIZE => {
                self.spare_resident_buffers.push(buffer);
            }
            _ => {}
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
    retention_bonus: u8,
    cold_strikes: u8,
    sampled_evictions: usize,
    sampled_refaults: usize,
}

impl AdaptiveController {
    fn initialize(&mut self) {
        if self.cold_strikes == 0 {
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
            self.retention_bonus = self.retention_bonus.saturating_add(1).min(7);
            self.cold_strikes = self.cold_strikes.saturating_add(1).min(4);
        } else if self.sampled_refaults.saturating_mul(100) < self.sampled_evictions {
            self.retention_bonus = self.retention_bonus.saturating_sub(1);
            self.cold_strikes = self.cold_strikes.saturating_sub(1).max(2);
        }

        self.sampled_evictions = 0;
        self.sampled_refaults = 0;
    }

    fn inspection_delay(&mut self, generation: u8) -> usize {
        self.initialize();
        debug_assert!(generation < RESIDENT_GENERATIONS);
        RESIDENT_BASE_DELAY
            + usize::from(generation) * RESIDENT_DELAY_STEP
            + usize::from(self.retention_bonus)
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

                if admission.generation == PROBATION_ADMISSION {
                    self.state.schedule_probation(Probation {
                        id: admission.id,
                        observed_at,
                    });
                } else {
                    let delay = self.state.controller.inspection_delay(admission.generation);
                    self.state.schedule_resident(
                        Resident {
                            id: admission.id,
                            observed_at,
                            generation: admission.generation,
                            cold_strikes: 0,
                        },
                        delay,
                    );
                }
                self.state.entry_count += 1;
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

        if self.state.epoch >= FEEDBACK_HORIZON {
            let expired_epoch = self.state.epoch - FEEDBACK_HORIZON;
            let slot = expired_epoch % FEEDBACK_HORIZON;
            let refaults = self.refaults_by_epoch[slot].swap(0, Ordering::Relaxed);
            let evictions = std::mem::take(&mut self.state.evictions_by_epoch[slot]);
            self.state.controller.observe(evictions, refaults);
        }

        self.state.queue_due_bucket();
        let budget = self.state.maintenance_budget(maintenance_floor);
        for _ in 0..budget {
            let Some(entry) = self.state.pop_due() else {
                break;
            };
            match entry {
                DueEntry::Probation(probation) => {
                    self.inspect_probation(probation, last_verified_at, evicted);
                }
                DueEntry::Resident(resident) => {
                    self.inspect_resident(resident, last_verified_at, evicted);
                }
            }
        }
    }

    fn inspect_probation(
        &mut self,
        probation: Probation,
        last_verified_at: &mut impl FnMut(Id) -> Option<Revision>,
        evicted: &mut Vec<Id>,
    ) {
        let Some(verified_at) = last_verified_at(probation.id) else {
            self.state.entry_count = self.state.entry_count.saturating_sub(1);
            return;
        };

        if verified_at > probation.observed_at {
            let generation = 0;
            let delay = self.state.controller.inspection_delay(generation);
            self.state.schedule_resident(
                Resident {
                    id: probation.id,
                    observed_at: verified_at,
                    generation,
                    cold_strikes: 0,
                },
                delay,
            );
        } else {
            self.record_eviction(probation.id, 0, evicted);
        }
    }

    fn inspect_resident(
        &mut self,
        mut resident: Resident,
        last_verified_at: &mut impl FnMut(Id) -> Option<Revision>,
        evicted: &mut Vec<Id>,
    ) {
        let Some(verified_at) = last_verified_at(resident.id) else {
            self.state.entry_count = self.state.entry_count.saturating_sub(1);
            return;
        };

        if verified_at > resident.observed_at {
            resident.observed_at = verified_at;
            resident.generation = resident
                .generation
                .saturating_add(1)
                .min(RESIDENT_GENERATIONS - 1);
            resident.cold_strikes = 0;
            let delay = self.state.controller.inspection_delay(resident.generation);
            self.state.schedule_resident(resident, delay);
            return;
        }

        if resident.generation > 0 {
            resident.generation -= 1;
            resident.cold_strikes = 0;
            let delay = self.state.controller.inspection_delay(resident.generation);
            self.state.schedule_resident(resident, delay);
            return;
        }

        resident.cold_strikes += 1;
        self.state.controller.initialize();
        if resident.cold_strikes < self.state.controller.cold_strikes {
            self.state.schedule_resident(resident, 1);
            return;
        }

        self.record_eviction(resident.id, 1, evicted);
    }

    fn record_eviction(&mut self, id: Id, return_generation: u8, evicted: &mut Vec<Id>) {
        debug_assert!(return_generation < RESIDENT_GENERATIONS);
        self.ghosts.record(id, self.state.epoch, return_generation);
        self.state.evictions_by_epoch[self.state.epoch % FEEDBACK_HORIZON] += 1;
        self.state.entry_count = self.state.entry_count.saturating_sub(1);
        evicted.push(id);
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
            .map(|(return_generation, _eviction_epoch)| {
                self.refaults_by_epoch[epoch % FEEDBACK_HORIZON].fetch_add(1, Ordering::Relaxed);
                return_generation
            })
            .unwrap_or(PROBATION_ADMISSION);
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
        if self.state.entry_count == 0 {
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

    fn record(&self, id: Id, epoch: usize, return_generation: u8) {
        let hash = hash(id);
        let fingerprint = (hash >> 32) as u32 | 1;
        let packed = Self::VALID
            | u64::from(fingerprint)
            | (((epoch & Self::EPOCH_MASK) as u64) << 32)
            | (u64::from(return_generation) << 56);
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
    fn probation_is_more_compact_than_a_resident() {
        assert!(std::mem::size_of::<Probation>() < std::mem::size_of::<Resident>());
    }

    #[test]
    fn cold_probation_ages_without_new_admissions() {
        let mut lru = Lru::with_shards(1, 2);
        let cold = id(0);
        let revision = Revision::start();

        lru.admit(cold);
        for _ in 1..PROBATION_DELAY {
            assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        }
        assert_eq!(lru.evict(revisions(&[(cold, revision)])), [cold]);
    }

    #[test]
    fn reused_probation_promotes_through_three_resident_generations() {
        let mut lru = Lru::with_shards(1, 2);
        let reused = id(0);

        lru.admit(reused);
        assert_eq!(lru.evict(revisions(&[(reused, Revision::start())])), []);
        for _ in 1..PROBATION_DELAY {
            assert_eq!(lru.evict(revisions(&[(reused, Revision::from(2))])), []);
        }
        assert_eq!(scheduled_generation(&lru, reused), Some(0));

        for _ in 0..RESIDENT_BASE_DELAY {
            assert_eq!(lru.evict(revisions(&[(reused, Revision::from(3))])), []);
        }
        assert_eq!(scheduled_generation(&lru, reused), Some(1));

        for _ in 0..RESIDENT_BASE_DELAY + RESIDENT_DELAY_STEP {
            assert_eq!(lru.evict(revisions(&[(reused, Revision::from(4))])), []);
        }
        assert_eq!(scheduled_generation(&lru, reused), Some(2));

        for _ in 0..RESIDENT_BASE_DELAY + 2 * RESIDENT_DELAY_STEP {
            assert_eq!(lru.evict(revisions(&[(reused, Revision::from(5))])), []);
        }
        assert_eq!(scheduled_generation(&lru, reused), Some(2));
    }

    #[test]
    fn cold_tenured_resident_demotes_before_eviction() {
        let mut lru = Lru::with_shards(1, 2);
        let cold = id(0);
        let revision = Revision::start();
        lru.state.schedule_resident(
            Resident {
                id: cold,
                observed_at: revision,
                generation: 2,
                cold_strikes: 0,
            },
            1,
        );
        lru.state.entry_count = 1;

        assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        assert_eq!(scheduled_generation(&lru, cold), Some(1));

        for _ in 0..RESIDENT_BASE_DELAY + RESIDENT_DELAY_STEP {
            assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        }
        assert_eq!(scheduled_generation(&lru, cold), Some(0));

        for _ in 0..RESIDENT_BASE_DELAY {
            assert_eq!(lru.evict(revisions(&[(cold, revision)])), []);
        }
        assert_eq!(lru.evict(revisions(&[(cold, revision)])), [cold]);
    }

    fn scheduled_generation(lru: &Lru, id: Id) -> Option<u8> {
        lru.state
            .resident_wheel
            .iter()
            .flatten()
            .find_map(|resident| (resident.id == id).then_some(resident.generation))
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

        for _ in 1..PROBATION_DELAY {
            assert_eq!(lru.evict(revisions(&entries)), []);
            assert_eq!(lru.state.due_count, 0);
        }

        assert_eq!(lru.evict(revisions(&entries)).len(), 4);
        assert_eq!(lru.state.due_count, 28);
        assert_eq!(lru.state.entry_count, 28);
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
            lru.state.entry_count
                < ADMISSIONS_PER_REVISION as usize
                    * (PROBATION_DELAY + TARGET_BACKLOG_REVISIONS + 4),
            "cold entry set did not converge: {}",
            lru.state.entry_count
        );
    }

    #[test]
    fn refault_feedback_increases_retention() {
        let mut controller = AdaptiveController::default();
        controller.observe(100, 20);

        assert_eq!(controller.retention_bonus, 1);
        assert_eq!(controller.cold_strikes, 3);
    }

    #[test]
    fn ghost_sketch_recognizes_recent_eviction() {
        let ghosts = GhostSketch::new(1);
        let id = id(0);
        ghosts.record(id, 4, 2);

        assert_eq!(ghosts.refault(id, 5), Some((2, 4)));
        assert_eq!(ghosts.refault(id, 67), Some((2, 4)));
        assert_eq!(ghosts.refault(id, 68), None);
    }

    #[test]
    fn a_probation_refault_bypasses_probation() {
        let mut lru = Lru::with_shards(1, 2);
        let candidate = id(0);
        let revision = Revision::start();

        lru.admit(candidate);
        for _ in 1..PROBATION_DELAY {
            assert_eq!(lru.evict(revisions(&[(candidate, revision)])), []);
        }
        assert_eq!(lru.evict(revisions(&[(candidate, revision)])), [candidate]);

        lru.admit(candidate);
        assert_eq!(
            lru.admissions[lru.shard(candidate)]
                .get_mut()
                .last()
                .unwrap()
                .generation,
            0
        );
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
