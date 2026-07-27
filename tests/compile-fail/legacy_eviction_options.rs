#[salsa::tracked(lru = 4000)]
fn legacy_lru(_db: &dyn salsa::Database) {}

fn main() {}
