#[salsa::tracked(eviction(policy = sieve))]
fn missing_capacity(_db: &dyn salsa::Database) {}

#[salsa::tracked(eviction(policy = random, capacity = 128))]
fn unknown_policy(_db: &dyn salsa::Database) {}

fn main() {}
