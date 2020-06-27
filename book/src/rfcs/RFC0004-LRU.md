# Summary

Add Least Recently Used values eviction as a supplement to garbage collection.

# Motivation

Currently, the single mechanism for controlling memory usage in salsa is garbage
collection. Experience with rust-analyzer shown that it is insufficient for two
reasons:

* It's hard to determine which values should be collected. Current
  implementation in rust-analyzer just periodically clears all values of
  specific queries.

* GC is in generally run in-between revision. However, especially after just
  opening the project, the number of values *within a single revision* can be
  high. In other words, GC doesn't really help with keeping peak memory usage
  under control. While it is possible to run GC concurrently with calculations
  (and this is in fact what rust-analyzer is doing right now to try to keep high
  water mark of memory lower), this is highly unreliable an inefficient.

The mechanism of LRU targets both of these weaknesses:

* LRU tracks which values are accessed, and uses this information to determine
  which values are actually unused.

* LRU has a fixed cap on the maximal number of entries, thus bounding the memory
  usage.

# User's guide

It is possible to call `set_lru_capacity(n)` method on any non-input query. The
effect of this is that the table for the query stores at most `n` *values* in
the database. If a new value is computed, and there are already `n` existing
ones in the database, the least recently used one is evicted. Note that
information about query dependencies is **not** evicted. It is possible to
change lru capacity at runtime at any time. `n == 0` is a special case, which
completely disables LRU logic. LRU is not enabled by default.

# Reference guide

Implementation wise, we store a linked hash map of keys, in the recently-used
order. Because reads of the queries are considered uses, we now need to
write-lock the query map even if the query is fresh. However we don't do this
bookkeeping if LRU is disabled, so you don't have to pay for it unless you use
it.

A slight complication arises with volatile queries (and, in general, with any
query with an untracked input). Similarly to GC, evicting such a query could
lead to an inconsistent database. For this reason, volatile queries are never
evicted.

# Alternatives and future work

LRU is a compromise, as it is prone to both accidentally evicting useful queries
and needlessly holding onto useless ones. In particular, in the steady state and
without additional GC, memory usage will be proportional to the lru capacity: it
is not only an upper bound, but a lower bound as well!

In theory, some deterministic way of evicting values when you for sure don't
need them anymore maybe more efficient. However, it is unclear how exactly that
would work! Experiments in rust-analyzer show that it's not easy to tame a
dynamic crate graph, and that simplistic phase-based strategies fall down.

It's also worth noting that, unlike GC, LRU can in theory be *more* memory
efficient than deterministic memory management. Unlike a traditional GC, we can
safely evict "live" objects and recalculate them later. That makes possible to
use LRU for problems whose working set of "live" queries is larger than the
available memory, at the cost of guaranteed recomputations.

Currently, eviction is strictly LRU base. It should be possible to be smarter
and to take size of values and time that is required to recompute them into
account when making decisions about eviction.
