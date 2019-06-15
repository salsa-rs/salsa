#![cfg(test)]

use super::*;
use linked_hash_map::LinkedHashMap;

#[derive(Debug)]
struct TestNode {
    id: usize,
    index: LruIndex,
}

impl TestNode {
    fn new(id: usize) -> Arc<Self> {
        Arc::new(TestNode {
            id,
            index: Default::default(),
        })
    }
}

impl LruNode for TestNode {
    fn lru_index(&self) -> &LruIndex {
        &self.index
    }
}

const LRU_SEED: &str = "Hello, Rustaceans";
const PICK_SEED: &str = "Wippity WIP";

/// Randomly requests nodes and compares the performance of a
/// *perfect* LRU vs our more approximate version. Since all the
/// random number generators use fixed seeds, these results are
/// reproducible. Returns (oracle_hits, lru_hits) -- i.e., the number
/// of times that the oracle had something in cache vs the number of
/// times that our LRU did.
fn compare(num_nodes: usize, capacity: usize, requests: usize) -> (usize, usize) {
    // Remember the clock each time we access a given element.
    let mut last_access: Vec<usize> = (0..num_nodes).map(|_| 0).collect();

    // Use a linked hash map as our *oracle* -- we track each node we
    // requested and (as the value) the clock in which we requested
    // it. When the capacity is exceed, we can pop the oldest.
    let mut oracle = LinkedHashMap::new();

    let lru = Lru::with_seed(LRU_SEED);
    lru.set_lru_capacity(capacity);

    let nodes: Vec<_> = (0..num_nodes).map(|i| TestNode::new(i)).collect();

    let mut oracle_hits = 0;
    let mut lru_hits = 0;

    let mut pick_rng = super::rng_with_seed(PICK_SEED);
    for clock in (0..requests).map(|n| n + 1) {
        let request_id: usize = pick_rng.gen_range(0, num_nodes);

        last_access[request_id] = clock;

        if oracle.contains_key(&request_id) {
            oracle_hits += 1;
        }

        if nodes[request_id].index.is_in_lru() {
            lru_hits += 1;
        }

        // maintain the oracle LRU
        oracle.insert(request_id, ());
        if oracle.len() > capacity {
            oracle.pop_front().unwrap();
        }

        // maintain our own version
        if let Some(lru_evicted) = lru.record_use(&nodes[request_id]) {
            assert!(!lru_evicted.index.is_in_lru());
        }
    }

    println!("oracle_hits = {}", oracle_hits);
    println!("lru_hits = {}", lru_hits);
    (oracle_hits, lru_hits)
}

#[test]
fn scenario_a() {
    let (oracle_hits, lru_hits) = compare(1000, 100, 10000);
    assert_eq!(oracle_hits, 993);
    assert_eq!(lru_hits, 973);
}
