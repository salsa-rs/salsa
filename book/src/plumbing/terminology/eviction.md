# Eviction

The `eviction` option on `#[salsa::tracked]` bounds the number of memoized values
retained for that function. The policy defaults to [SIEVE]. To use LRU instead,
set `policy = lru`.

SIEVE is recommended because its lock-free cache-hit path supports higher
throughput under concurrency without serializing on the eviction policy. The
[SIEVE paper] also found miss ratios comparable to or better than more complex
eviction policies. SIEVE can favor repeatedly accessed entries over one-hit
wonders, but tracks approximate rather than exact recency and may inspect
several residents when choosing a victim. Results remain workload-dependent.

LRU maintains exact recency. Salsa's current implementation guards that order
with an exclusive mutex. Every query access, including a cache hit, takes the
mutex and updates the order, so concurrent hits to otherwise independent entries
serialize.

A configured function generates
[`function_name::set_eviction_capacity(&mut db, capacity)`](https://docs.rs/salsa/latest/salsa/attr.tracked.html)
to adjust its capacity at runtime.

Eviction drops values from older [memos] to conserve memory, but retains their
[dependency] information so Salsa can still determine whether dependent values
may have changed. See [cache eviction] for configuration examples.

[cache eviction]: ../../tuning.md#cache-eviction
[dependency]: ./dependency.md
[memos]: ./memo.md
[SIEVE]: https://cachemon.github.io/SIEVE-website/
[SIEVE paper]: https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo
