#[salsa::interned(eviction())]
struct EmptyEviction {
    field: u32,
}

#[salsa::interned(eviction(policy = random))]
struct UnknownPolicy {
    field: u32,
}

#[salsa::interned(eviction(policy = lru, policy = no_eviction))]
struct DuplicatePolicy {
    field: u32,
}

#[salsa::interned(eviction(revisions = 1, revisions = 2))]
struct DuplicateRevisions {
    field: u32,
}

#[salsa::interned(eviction(capacity = 12))]
struct UnsupportedCapacity {
    field: u32,
}

#[salsa::interned(eviction(unknown = 12))]
struct UnknownField {
    field: u32,
}

#[salsa::interned(eviction(policy = lru), eviction(policy = no_eviction))]
struct DuplicateEviction {
    field: u32,
}

#[salsa::interned(revisions = 1, eviction(revisions = 2))]
struct RevisionsInsideAndOutside {
    field: u32,
}

#[salsa::interned(eviction(policy = no_eviction, revisions = 2))]
struct NoEvictionWithNestedRevisions {
    field: u32,
}

#[salsa::interned(revisions = 2, eviction(policy = no_eviction))]
struct NoEvictionWithLegacyRevisions {
    field: u32,
}

#[salsa::input(eviction(policy = no_eviction))]
struct InputWithEviction {
    field: u32,
}

#[salsa::tracked(eviction(policy = no_eviction))]
fn tracked_with_eviction(_db: &dyn salsa::Database) {}

#[salsa::tracked(eviction(policy = no_eviction))]
struct TrackedWithEviction {
    field: u32,
}

#[salsa::accumulator(eviction(policy = no_eviction))]
struct AccumulatorWithEviction(u32);

fn main() {}
