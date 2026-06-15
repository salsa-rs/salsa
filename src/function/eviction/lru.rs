//! TinyLFU admission with approximate LRU eviction.
//!
//! Access frequencies are recorded without taking the resident-set lock. New
//! values are buffered in admission shards and compete with the least recently
//! used resident at the start of a revision. Resident promotions are
//! best-effort so a contended cache never blocks a query fetch.

use std::hash::BuildHasher;
use std::num::NonZeroUsize;

use crossbeam_utils::CachePadded;
use rustc_hash::FxBuildHasher;

use crate::Id;
use crate::hash::FxLinkedHashSet;
use crate::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use crate::sync::{Mutex, OnceLock};

use super::{EvictionPolicy, HasCapacity};

const SKETCH_DEPTH: usize = 4;
const MAX_FREQUENCY: u8 = 15;
const SAMPLE_MULTIPLIER: usize = 10;

/// TinyLFU admission backed by an approximately ordered LRU resident set.
pub struct Lru {
    capacity: Option<NonZeroUsize>,
    hasher: FxBuildHasher,
    shift: u32,
    residents: Mutex<FxLinkedHashSet<Id>>,
    admissions: Box<[CachePadded<Mutex<Vec<Id>>>]>,
    sketch: FrequencySketch,
    next_admission_shard: usize,
}

impl Lru {
    fn with_shards(capacity: usize, shards: usize) -> Self {
        assert!(shards > 1 && shards.is_power_of_two());

        Self {
            capacity: NonZeroUsize::new(capacity),
            hasher: FxBuildHasher,
            shift: usize::BITS - shards.trailing_zeros(),
            residents: Mutex::default(),
            admissions: (0..shards).map(|_| Default::default()).collect(),
            sketch: FrequencySketch::new(capacity),
            next_admission_shard: 0,
        }
    }

    #[inline]
    fn shard(&self, id: Id) -> usize {
        let hash = self.hasher.hash_one(id);
        ((hash as usize) << 7) >> self.shift
    }

    #[inline(never)]
    fn push_admission(&self, id: Id) {
        self.admissions[self.shard(id)].lock().push(id);
    }

    fn consider_candidate(
        sketch: &FrequencySketch,
        residents: &mut FxLinkedHashSet<Id>,
        capacity: usize,
        candidate: Id,
        cb: &mut impl FnMut(Id),
    ) {
        if residents.to_back(&candidate) {
            return;
        }

        if residents.len() < capacity {
            residents.insert(candidate);
            return;
        }

        let victim = *residents
            .front()
            .expect("a full TinyLFU resident set has no victim");
        if sketch.frequency(candidate) > sketch.frequency(victim) {
            residents.pop_front();
            residents.insert(candidate);
            cb(victim);
        } else {
            cb(candidate);
        }
    }
}

impl EvictionPolicy for Lru {
    fn new(capacity: usize) -> Self {
        static SHARDS: OnceLock<usize> = OnceLock::new();
        let shards = *SHARDS.get_or_init(|| {
            let parallelism = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);
            (parallelism * 4).next_power_of_two()
        });

        Self::with_shards(capacity, shards)
    }

    #[inline(always)]
    fn admit(&self, id: Id) {
        if self.capacity.is_some() {
            self.push_admission(id);
        }
    }

    #[inline(always)]
    fn promote(&self, id: Id) {
        if self.capacity.is_none() {
            return;
        }

        self.sketch.increment(id);
        if let Some(mut residents) = self.residents.try_lock() {
            residents.to_back(&id);
        }
    }

    fn set_capacity(&mut self, capacity: usize) {
        self.capacity = NonZeroUsize::new(capacity);
        self.sketch = FrequencySketch::new(capacity);

        if self.capacity.is_none() {
            self.residents.get_mut().clear();
            for admissions in &mut self.admissions {
                admissions.get_mut().clear();
            }
            self.next_admission_shard = 0;
        }
    }

    fn for_each_evicted(&mut self, mut cb: impl FnMut(Id)) {
        let Some(capacity) = self.capacity.map(NonZeroUsize::get) else {
            return;
        };

        self.sketch.age_if_needed(capacity);

        let residents = self.residents.get_mut();
        while residents.len() > capacity {
            cb(residents.pop_front().expect("non-empty resident set"));
        }

        let admissions: Vec<Vec<Id>> = self
            .admissions
            .iter_mut()
            .map(|shard| std::mem::take(shard.get_mut()))
            .collect();
        let mut cursors = vec![0; admissions.len()];
        let mut remaining = admissions.iter().map(Vec::len).sum::<usize>();

        while remaining > 0 {
            for offset in 0..admissions.len() {
                let shard = (self.next_admission_shard + offset) & (admissions.len() - 1);
                let Some(&candidate) = admissions[shard].get(cursors[shard]) else {
                    continue;
                };

                cursors[shard] += 1;
                remaining -= 1;
                Self::consider_candidate(&self.sketch, residents, capacity, candidate, &mut cb);
            }
            self.next_admission_shard = (self.next_admission_shard + 1) & (admissions.len() - 1);
        }

        for (shard, mut admissions) in self.admissions.iter_mut().zip(admissions) {
            admissions.clear();
            *shard.get_mut() = admissions;
        }
    }
}

impl HasCapacity for Lru {}

struct FrequencySketch {
    width: usize,
    counters: Box<[AtomicU8]>,
    samples: AtomicUsize,
}

impl FrequencySketch {
    fn new(capacity: usize) -> Self {
        let width = capacity.next_power_of_two().max(16);
        Self {
            width,
            counters: (0..width * SKETCH_DEPTH)
                .map(|_| AtomicU8::new(0))
                .collect(),
            samples: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn increment(&self, id: Id) {
        let hash = hash(id);
        for row in 0..SKETCH_DEPTH {
            let counter = &self.counters[self.index(hash, row)];
            let mut current = counter.load(Ordering::Relaxed);
            while current < MAX_FREQUENCY {
                match counter.compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => current = actual,
                }
            }
        }
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn frequency(&self, id: Id) -> u8 {
        let hash = hash(id);
        (0..SKETCH_DEPTH)
            .map(|row| self.counters[self.index(hash, row)].load(Ordering::Relaxed))
            .min()
            .unwrap_or(0)
    }

    fn age_if_needed(&mut self, capacity: usize) {
        if *self.samples.get_mut() < capacity.saturating_mul(SAMPLE_MULTIPLIER) {
            return;
        }

        for counter in &mut self.counters {
            *counter.get_mut() /= 2;
        }
        *self.samples.get_mut() /= 2;
    }

    #[inline]
    fn index(&self, hash: u64, row: usize) -> usize {
        const SEEDS: [u64; SKETCH_DEPTH] = [
            0x9e37_79b9_7f4a_7c15,
            0xc2b2_ae3d_27d4_eb4f,
            0x1656_67b1_9e37_79f9,
            0x85eb_ca77_c2b2_ae63,
        ];
        let mixed = mix(hash ^ SEEDS[row]);
        row * self.width + (mixed as usize & (self.width - 1))
    }
}

#[inline]
fn hash(id: Id) -> u64 {
    mix(id.as_bits())
}

#[inline]
fn mix(mut value: u64) -> u64 {
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

    fn evicted(lru: &mut Lru) -> Vec<Id> {
        let mut evicted = Vec::new();
        lru.for_each_evicted(|id| evicted.push(id));
        evicted
    }

    #[test]
    fn frequent_candidate_replaces_lru_victim() {
        let mut lru = Lru::with_shards(1, 2);
        let resident = id(0);
        let candidate = id(1);

        lru.admit(resident);
        lru.promote(resident);
        assert_eq!(evicted(&mut lru), []);

        lru.admit(candidate);
        lru.promote(candidate);
        lru.promote(candidate);
        assert_eq!(evicted(&mut lru), [resident]);
    }

    #[test]
    fn infrequent_candidate_is_rejected() {
        let mut lru = Lru::with_shards(1, 2);
        let resident = id(0);
        let candidate = id(1);

        lru.admit(resident);
        lru.promote(resident);
        lru.promote(resident);
        assert_eq!(evicted(&mut lru), []);

        lru.admit(candidate);
        lru.promote(candidate);
        assert_eq!(evicted(&mut lru), [candidate]);
    }

    #[test]
    fn sketch_ages_frequencies() {
        let mut sketch = FrequencySketch::new(1);
        let id = id(0);
        for _ in 0..MAX_FREQUENCY {
            sketch.increment(id);
        }

        sketch.age_if_needed(1);

        assert_eq!(sketch.frequency(id), MAX_FREQUENCY / 2);
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

        lru.admit(candidate);
        let capacity = lru.admissions[shard].get_mut().capacity();
        assert!(capacity > 0);

        assert_eq!(evicted(&mut lru), []);
        assert_eq!(lru.admissions[shard].get_mut().capacity(), capacity);
    }
}
