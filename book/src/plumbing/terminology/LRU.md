# LRU

The [`set_lru_capacity`](https://docs.rs/salsa/0.16.1/salsa/struct.QueryTableMut.html#method.set_lru_capacity) method can be used to fix the maximum capacity for a query at a specific number of values. If more values are added after that point, then salsa will drop the values from older [memos] to conserve memory (we always retain the [dependency] information for those memos, however, so that we can still compute whether values may have changed, even if we don't know what that value is).

[memos]: ./memo.md
[dependency]: ./dependency.md
