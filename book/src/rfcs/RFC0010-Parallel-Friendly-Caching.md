# Description/title

## Metadata

* Author: nikomatsakis
* Date: 2021-05-29
* Introduced in: (please update once you open your PR)

## Summary

* Create a separate cache that stores only completed items.
* Move cycle detection to be per-thread.
* Introduce a special 

## Motivation

* To prepare for a more "parallel by default" future:
    * Eventual goal: Cache hit requires only reads (no locks).
    * Validation and execution require no locks and few atomic writes.
        * Although synchronization remains an option.
    * This RFC brings that within sight, although it is not achieved.
* To prepare for better cycle handling, where caching is less "all or nothing"
    * To support chalk-style solving, we need caching of provisional results.
    * Also want more graceful ability to recover from cycles.

## User's guide

### Phase 1 (this RFC)

No user visible change

### Phase 2 (future RFCs)

* Users can declare fixed point queries that re-execute cycles
    * Enables salsa integration
* Users can declare return value in case of cycles to prevent panics
* Salsa governs a thread pool to run speculative and future work
* Create forked databases that contain speculative changes
* Queries can run in parallel unless marked as synchronized
    * We could opt for an alternative default

## Reference guide

### Phase 1 (this RFC)

The overall structure should look like

* Each `Storage<DB>` has
    * handle to the `Arc<SharedQueryQuery<DB>>`
    * its own `LocalStorage<DB>`
    * a `Runtime`
* Each query has
    * a `SharedStorage<Q>` found in the `SharedQueryQuery<DB>`:
        * monotonically growing map from keys (`Q::Key`) to internal index `X`
        * lru set
        * map from internal index `X` to a cached memo `Arc<Memo<Q>>`
        * synchronized map from internal index `X` to thread id + latch
    * a `LocalStorage<Q>` found in the `LocalStorage<DB>`
        * map from internal index `X` to stack depth
* A `Memo<Q>` contains a `Option<Q::Value>` and dependency information
    * This information is (largely) immutable with a few `AtomicCell` instances.

To execute a query `Q`:

* Find the internal index `X` (read lock)
* Check the shared query storage for `X` (read lock) and extract (optional) memo `M`.
* If a memo `M` was found and has a value:
    * Validate the memo `M`. Return clone of value if valid.
* Check local storage to see if this query is already executing
    * If so, recover from cycle and return
* If synchronized query (always true in this RFC):
    * Insert into synchronization map. If an entry is already present:
        * Wait on the latch and start again when it is complete.
* Push on the local stack
* Push index on local stack into local storage
* Execute the query function
    * Result is: a value V, a set of dependents D, and a minimum stack depth M
* If the minimum stack depth is less than the current stack depth, do not move the result into the full cache.
    * It can stay in the local storage, but it will have to be cleared out when `D` is complete. For now, let's pop it.

To recover from a cycle on a query `Q`:

To validate an internal index `X`:

* Load and validate the memo `M`

To validate a memo `M` (returns false if may have changed in this revision):

* Load the `verified_at` revision `V`. If it is the current revision, return true ("no change")
* If the "last change" `LC` revision for `M.durability` is greater than `V` (and hence we may have changed):
    * Iterate over the dependences in `M` and check if they have changed since `V`
        * If so, return false ("maybe changed")
            * Should we set `verified_at` to INT_MAX or some marker value?
* Adjust `verified_at` to the current revision, return true ("maybe changed")

### Phase 2 (future RFCs)

This is just a sketch of what will be needed.

* For fixed-point queries, the local storage will contain a map to an in-progress value along with information about what stack depth it depends on.
* For regular queries, the local will contain a map to a cycle value declared by user (if any).
* The following maps can be made more optimal:
    * Key `Q::Key` to internal index `X` map
        * May have removals/writes if data is from older revision
    * Internal index `X` to memo map
        * Purely monotonic (should we fix that?)

### Older material

* A `Database` (per thread) contains a
    * Storage<DB>
* A `Storage<DB>` (per thread) contains a
    * `Arc<SharedStorage<DB>>`
    * `LocalStorage<DB>`
    * `Runtime`
* A `SharedStorage<DB>` contains
    * `SharedQueryStorage<Q>` for each query `Q` in `DB`
* A `SharedQueryStorage<Q>` contains
    * `RwLock<FxIndexSet<Q::Key>>` to map from key to internal index `X`
    * lru information
    * a map from internal index `X` to a `Memo<Q>`
* A `Memo<Q>` contains
    * a `Option<Q::Value>` -- can we represent this more efficiently?
    * lru information (perhaps)
    * a `MemoRevisions` containing
        * `verified_at: Revision`
        * `changed_at: Revision`
        * `durability: Revision`
        * `inputs: MemoInputs`
* A `LocalStorage<DB>` contains
    * `LocalQueryStorage<Q>` for each query `Q` in `DB`
* A `LocalQueryStorage<Q>` contains
    * a map from an internal index `X` to a `LocalMemo<Q>`
* A `LocalMemo<Q>` contains
    * an optional value `Q::Value`
    * an optional index into the stack
    * some kind of upper bound, maybe an `Rc<Cell<usize>>`?, indicating how high on the stack it goes
* A `Runtime` contains
    * `SharedRuntimeState`
    * `LocalRuntimeState`
* `SharedRuntimeState` contains
    * data for synchronized queries
* `LocalRuntimeState` contains
    * `RefCell<Vec<MemoizedValue>>`

* To perform a query
    * Check global cache. If entry found:
        * Validate. If validation successful:
            * Return the result.
    * If this is a fixed point query Q:
        * Check local cache.
    * Otherwise:
        * Check if query is on the stack at depth D.
        * 
    * Check local cache. If entry found:
        * Check if it is on the stack at index S0. If so:
            * (handle cycle XXX)
        * Not on stack. Return provisional value and register a read at depth within cache.
    * Insert a local cache

* How do cycles work?
    * When we find a query, we check the local cache
    * If there is a local memo and the memo is on the stack:
        * Check if there is a value. If not, panic with a cycle error.
        * If this is a "fixed point" query and all things on the stack are the same query:
            * How to manage inductive/coinductive cycles? Could have different goals.
            * Otherwise, we have some kind of callback to generate the result
                * It can be given a `Iterator<Q::Key>`
                * And the current cached value
        * If not a fixed point query or mixed queries:
            * Can have a "cycle error" result somehow, maybe
* How do synchronized queries work?
* Tag the query synchronized
    * It will call `runtime.synchronized(|| ...)` at the start or whatever
    * Manage cycles

```rust
type K = u32;

/// For each query, we have a `QueryStorage` struct.
struct DerivedStorage<Q: Query> {
    /// used to construct a `DatabaseKeyIndex`
    key_map: RwLock<FxIndexSet<Q::Key>>,
    lru_list: ...,
    cache: RwLock<FxHashMap<K, MemoizedValue<Q>>>
}

/// Storage for things being actively executed
struct StackStorage<Q: Query> {
    /// used to construct a `DatabaseKeyIndex`
    provisional_cache: FxHashMap<K, MemoizedStackValue<Q>>,
}

struct MemoizedValue<Q>
where
    Q: QueryFunction,
{
    /// The result of the query, if we decide to memoize it.
    value: Option<Q::Value>,

    /// Revision information
    revisions: MemoRevisions,
}
```

* How to get the per-thread storage?
* Not THAT many options
* Right now the Storage 

## Frequently asked questions

Use this section to add in design notes, downsides, rejected approaches, or other considerations.

