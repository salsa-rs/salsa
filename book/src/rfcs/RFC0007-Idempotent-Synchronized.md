# Description/title

## Metadata

* Author: nikomatsakis
* Date: 2020-07-13
* Introduced in: https://github.com/salsa-rs/salsa/pull/1 (please update once you open your PR)

## Summary

* Divide queries between "idempotent" (can execute more than once) and "synchronized" (executes at most once).
* Queries are idempotent by default, must be marked as `#[synchronized]`.
* Simplify caching structure.

## Motivation

This RFC proposes to simplify the caching structure for queries, with the
following goals:

* Improve performance in parallel situations, so that we can execute queries
  without any specific locks.
* Simplify the overall design of the system to make it easier to follow along.
* Lay the groundwork for supporting cycles involving monotonic queries, such
  as will be needed to integrate the Chalk solver.

### Idempotent and synchronized queries

A big part of what makes our caching logic complex is that we currently
guarantee "at most once' execution for most queries `Q(K)`. This means that when
a query `Q(K)` begins executing, we have to install a "placeholder" so that if
other threads come along later, they can block until the original thread
completes. This guarantee is, however, not universal: queries marked with
`#[dependencies]` are re-executed on each use.

In practice, it's not clear that blocking on other threads is a good choice much
of the time. Many queries are relatively lightweight and it would be harmless to
simply execute them more than once if necessary.

## User's guide

### Categories of queries

The proposal is to have four categories of cached queries:

* `#[transparent]` -- stores no information, really just a function;
* `#[dependencies]` -- stores just dependency information, re-executes query
  on every invocation;
* `#[cached]` -- **the default**, caches both dependency information and value,
  but does not guarantee "at most once" execution.
* `#[synchronized]` -- as the previous option, but also guarantees at most once
  execution.

| Query type | Tracks deps? | Caches value? | At most once |
| --- | --- | --- | --- |
| Transparent | ❌ | ❌ | ❌ |
| Dependencies | ✔️ | ❌ | ❌ |
| **Cached** (default) | ✔️ | ✔️ | ❌ |
| Synchronized | ✔️ | ✔️ | ✔️ |

### When do you want to synchronize?

There are two reasons to synchronize queries:

* Not deterministic: Queries that are not fully deterministic need to be
  synchronized. If executing the query twice might yield truly different
  results, the system will behave in unpredictable ways. An example of a
  non-deterministic query might be a query that executes a user supplied
  procedural macro. Marking such a query as synchronized will ensure that we
  execute the macro at most once and use the result.
* Performance: If a query is expensive, it might be better to block waiting
  for the result than to execute it twice.

## Reference guide

### Derived query structures

The plan is for derived queries to have the following maps
and structures:

* `writer_mutex: Mutex<()>` -- a mutex that is used only when registering
  a new key
* `key_indices: DashMap<Q::Key, u32>` -- this maps from a key to an index in the
  `key_data` slab.
* `key_data: ShardedSlab<KeyData<Q>>` -- this permits indexed access to a
  particular key with no locks.
* the `KeyData<Q>` struct stores the following information for each key:
  * the `Q::Key` value
  * an `ArcSwapOption<Memo<Q::Value>>` storing the memoized value/dependencies
* a `Memo<V>` combines a (optional) memoized value and its dependencies, and stores:
  * the value `Option<V>`
  * a `MemoRevisions` roughly equivalent to today, but with an `AtomicCell<Revision>`
    for the verified-at field
  * a lru-index field for use with the LRU code (only used if the value is provided)
* a `synchronization: Mutex<FxHashMap<usize, InProgress>>` map used for `#[synchronized]` queries,
  but otherwise unused, where `InProgress` stores the same data as today's `QueryState::InProgress` variant

### Runtime structures

Within the runtime, we store the active query stack in a
`FxIndexMap<DatabaseKeyIndex, ActiveQuery>`. This permits fast access to the
current query (it's the "last" item in the map) and also quick detection of
"intra-thread cycles" (if there is already a value for a given key, that's a
problem). Since this stack is thread-local, use of an index map is not a
problem.

### Executing a query

Executing a query `db.query(key)` now takes the following steps,
both of which are described in more detail below:

* Create an index for `key` if one does not already exist.
* If there is a memo, check if the dependencies are still valid and return the enclosed value, if so.
* Compute the value given the index.

### Create an index for a key

To get the index for a key, we take the following steps:

* Look in the `key_indices` map and return the index if it exists
* Otherwise, acquire the `writer_mutex` and:
  * Look in the `key_indices` map and return the index if it exists
  * If not, push a new (empty) entry into the `key_data` slab
  * Store the resulting index into `key_indices`

### Compute value given index

The procedure to compute the query value, given the index, is as follows:

* If this is a synchronized query, then try to claim it. If already claimed, then block on the resulting promise and return.
* Push the current query index onto the "active query" stack:
  * If a cycle is detected, initiative cycle recovery and return.
* Execute the query, resulting in a value + dependency information.
* Pop the query from the active query stack.
* Store a new memo into the `key_data`, unless another version has been stored in the meantime.
* If this is a synchronized query, inform any blocked queries they can re-execute.
* Return the result.

**Races between writers:** There may be multiple writers at once. We assume
that if multiple values are computed, we need to store one of them as "the memo",
but it doesn't matter too much which one.

### Verifying dependencies

Verifying dependencies can be done roughly as today. If a memoized value is
available, then we first load the revision `V` when it was last verified. If
this is the current revision, we can immediate return. Otherwise, walk the
inputs found along with it. These are immutable so they can be iterated simply.
For each input `I: DatabaseKeyIndex`, we invoke `db.maybe_changed_since(I, V)`
to indicate. If this is false for all inputs, then verification succeeds. In
this case, the `verified_at` field can be set to the current revision (this may
be done by other threads in parallel, as well).

### Computing whether a query has changed since the revision R

Besides executing a query, the other core operation is executing
`maybe_changed_since(i, r)` for some index `i` and some revision `r`. Under this
new scheme, the index `i` can be used to directly access the slab and check for
a memoized value. Computing whether a query has changed since `r` begins with
verifying dependencies. If verification fails, and there is a memoized value available
whose `changed_at` is earlier than `r`, then we can compute the
value for the current index (as described earlier) and see whether that value
is the same as the memoized value (i.e., is the value backdated). If so, 
we can return that the query has not changed.

### Evicting for LRU

The LRU system (for now, at least) can continue to work roughly as it does
today, but with a few tweaks. For one thing, the LRU system today stores a
`Arc<Node>` values that directly reference slots. We would instead store the key
index values (i.e., the `usize`) that index into the slab. This will require
integrating the LRU a bit more closely into the query storage so that, given an
index, it can access and modify the LRU index data.

### Data structure choices and requirements

This section discusses the choice of crates and the actual requirements
for each datastructure. 

For `key_indices`, the [dashmap] crate appears to be the best concurrent hashmap
available. It is more general than we need in that it supports multiple parallel
writers, but we only ever have one, and it supports removal of keys and other
such features. On the other hand, it is also less capable than we might like, in
that unlike `IndexMap` it doesn't support indexing (and hence we must store the
`Q::Key` in the `key_data` array).

For `key_data`, shared-slab is used but it has several capabilities we don't
require. We only have a single writer at a time (but that writer executes in
parallel with readers). We don't remove entries and don't really require
generational indexing.

[dashmap]: https://crates.io/crates/dashmap

## Alternatives and future work

### Dropping cached values eagerly

This RFC does not address the desire, expressed by maklad in the (unmerged)
[better defaults RFC], to have queries whose cached values are automatically
dropped when we enter the next revision. This could be readily accommodated as
an extension to the hierarchy, though I'm unsure if this problem was sufficiently
addressed by the need for LRU. 

[better defaults RFC]: https://github.com/salsa-rs/salsa-rfcs/pull/4

### Values that are not `Eq`

This RFC does not address the problem that we cannot accommodate queries whose
values do not implement `Eq` but which *are* cached. We used to permit this
with volatile queries, but we no longer support those. Note that if we permitted
values to be cached but only within one revision, this would address the same
use cases as volatile (particularly combined with volatile reads).

### Backdating with hashes

Another way to handle "backdating" besides using `Eq` is to permit storing a
cryptographic hash of the value instead of the value itself, which would permit
us to do a better job determining if the value changed in the new revision, even
if the value itself must be recomputed if needed.

### Synchronization mid-query

The current setup requires that each query be fully synchronized or not. But in
cases where synchronization is a performance optimization, we might want to be
able to only synchronize *sometimes* -- e.g., if one were to synchronize
type-checking, perhaps it would only be done for "big" functions past some
threshold.