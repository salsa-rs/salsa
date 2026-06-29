# LRU

The `lru` option on `#[salsa::tracked]` fixes the maximum number of values retained for that function. Functions with `lru` also generate [`function_name::set_lru_capacity(&mut db, capacity)`](https://docs.rs/salsa/latest/salsa/attr.tracked.html) to adjust the capacity. Salsa drops values from older [memos] to conserve memory, but retains their [dependency] information so that it can still compute whether values may have changed. See [cache eviction](../../tuning.md#cache-eviction-lru) for examples.

[memos]: ./memo.md
[dependency]: ./dependency.md
