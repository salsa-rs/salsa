# Parallel friendly caching

## Metadata

* Author: nikomatsakis
* Date: 2021-05-29
* Introduced in: (please update once you open your PR)

## Summary

* Split local/global query state
    * Local query state is specific to each worker thread
    * Global query state is used across all worker threads
* Global query state will store:
    * Monotonically growing map from key to Slot Id (an integer)
    * Completed query results (and their dependencies)
        * This is a simple data structure, which permits a query hit to complete without any atomic writes
    * Where appropriate, a "synchronization set", storing the indices of queries that are currently executing
        * This is optional, which permits us to have the same query running multiple times in parallel if desired
* Local query state will store:
    * Provisional cached items, in the case of fixed-point cycles (needed by chalk; not covered in this RFC)

## Motivation

Two-fold:

* To integrate chalk caching with salsa more deeply, we need to be able to handle cycles more gracefully.
    * In chalk, cycles are handled by iterating until a fixed-point is reached. This could be useful in other salsa applications as well.
    * For this to work, we need caching of *provisional results* that are produced during those iterations. These results should not be exposed outside the cycle.
* We are moving towards a 'parallel by default' future for Salsa.   
    * Eventual goal: Cache hit requires only reads (no locks).
    * Creates the option of queries where a cache hit requires no atomic writes.
    * Validation and execution require no locks and few atomic writes.
        * Although synchronization remains an option.
    * This RFC brings that within sight, although it is not achieved.

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

### Phase 0 (design before this RFC)

Today, the **overall structure** looks like this (some generic parameters and details elided):

* Each `Storage<DB>` has
    * a `Runtime` (per-thread runtime),
    * handle to the `Arc<DB::DatabaseStorage>` (global storage for all queries).
* Each `Runtime` has
    * its own `LocalState` which contains the query stack,
    * handle to the `Arc<SharedState>` which contains cross-thread dependency information.
* Each query has
    * an `impl QueryStorageOps<Q>` that defines [maybe changed since] and [fetch].
    * *Input* and *interned* queries have
        * *In `DB::DatabaseStorage`:* hashmaps from key -> value
    * *Derived* queries have:
        * *In `DB::DatabaseStorage`:* a `DerivedStorage<Q>` that contains:
            * the `slot_map`, a monotonically growing, indexable map from keys (`Q::Key`) to the `Slot<Q>` for the given key
            * lru list
* Each `Slot<Q>` has
    * r-w locked query-state that can be:
        * not-computed
        * in-progress with synchronization storage:
            * `id` of the runtime computing the value
            * `anyone_waiting`: `AtomicBool` set to true if other threads are awaiting result
        * a `Memo<Q>`
* A `Memo<Q>` has
    * an optional value `Option<Q::Value>`
    * dependency information:
        * verified-at
        * changed-at
        * durability
        * input set (typically a `Arc<[DatabaseKeyIndex]>`)

[maybe changed since]: ../plumbing/maybe_changed_since.md
[fetch]: ../plumbing/fetch.md

**Fetching the value for a query** works as follows:

* *Derived* queries:
    * Acquire the read lock on the (indexable) `slot_map` and hash key to find the slot.
        * If no slot exists, acquire write lock and insert.
    * Acquire the slot's internal lock to perform the [fetch] operation.
* *Input* queries:
    * Acquire the read lock on the (indexable) `slot_map` and hash key to find the slot.
        * If no slot exists, acquire write lock and insert.
    * Acquire the write lock on the slot to modify the value.
* *Interned* queries:
    * Acquire the read lock on the (indexable) `slot_map` and hash key to find the slot.
        * If slot exists, return the corresponding index.
        * If no slot exists, acquire write lock and insert new memo.

**Verifying a dependency** uses a scheme introduced in [RFC #6](./RFC0006-Dynamic-Databases.md). Each dependency is represented as a `DatabaseKeyIndex` which contains three indices (group, query, and key). The group and query indices are used to find the query storage via `match` statements and then the next operation depends on the query type:

* *Derived* queries:
    * Acquire the read lock on the (indexable) `slot_map` and use key index to load the slot. Read lock is released afterwards.
    * Acquire the slot's internal lock to perform the [maybe changed since] operation.
* *Input* queries:
    * Acquire the read lock on the (indexable) `slot_map` and use key index to load the slot. Read lock is released afterwards.
    * Acquire the read lock on the slot to read the 'last changed' revision.
* *Interned* queries:
    * Acquire the read lock on the (indexable) `slot_map` and use key index to load the slot. Read lock is released afterwards.
    * Read the 'last changed' revision directly: slots for interned queries are immutable, so no locks are needed.

### Phase 1 (this RFC)

The **overall structure** we are creating looks like this.

* Each `Storage<DB>` has
    * a `Runtime` (per-thread runtime),
    * its own `LocalStorage<DB>` (per-thread storage for all queries),
        * This *local storage* can be used to store intermediate results.
    * handle to the `Arc<GlobalQueryStorage<DB>>` (global storage for all queries).
        * This *global storage* is used to store completed results.
* Each `Runtime` has
    * its own `LocalState` which contains the query stack,
    * handle to the `Arc<SharedState>` which contains cross-thread dependency information.
* Each query has
    * an `impl LocalQueryStorageOps<Q>` that defines [maybe changed since] and [fetch]
        * these "local" ops can access the global storage as needed
    * *Input* and *interned* queries have:
        * Local storage (`LocalStorage<DB>`): phantom data
        * Global storage (`GlobalQueryStorage<DB>`): hashmaps from key -> value
    * *Derived* queries have:
        * a `LocalStorage<Q>` found in the `LocalStorage<DB>`
            * map from internal index `X` to stack depth
        * an `SharedStorage<Q>` found in the `GlobalQueryStorage<DB>`:
            * various hashmaps, all implemented as concurrent hashmaps (e.g., via [`DashMap`](https://crates.io/crates/dashmap)):
                * `key_map`: maps from `Q::Key` to an internal key index `K`
                * `memo_map`: maps from `K` to cached memo `Arc<Memo<Q>>`
                * `sync_map`: maps from `K` to a `Sync<Q>` synchronization value
            * lru set with `Memo<Q>` as the nodes
* A `Memo<Q>` has
    * an *immutable* optional value `Option<Q::Value>`
    * dependency information:
        * *updatable* verified-at (`AtomicCell<Option<Revision>>`)
        * *immutable* changed-at (`Revision`)
        * *immutable* durability (`Durability`)
        * *immutable* input set (typically a `Arc<[DatabaseKeyIndex]>`)
    * information for LRU:
        * `DatabaseKeyIndex`
        * `lru_index`, an `AtomicUsize`
* A `Sync<Q>` has
    * `id` of the runtime computing the value
    * `anyone_waiting`: `AtomicBool` set to true if other threads are awaiting result

**Fetching the value for a *derived* query** will work as follows. Fetching values for *interned* and *input* queries is unchanged.

1. Find internal index `K` by hashing key, as today.
    * Precise operation for this will depend on the concurrent hashmap implementation.
2. Load memo `M: Arc<Memo<Q>>` from `memo_map[K]` (if present):
    * If verified is `None`, then another thread has found this memo to be invalid; ignore it.
    * Else, let `Rv` be the "last verified revision".
    * If `Rv` is the current revision, **return** memoized value.
    * If last change to an input with durability `M.durability` was before `Rv`: 
        * Update `verified_at` to current revision and **return** memoized value.
    * Iterate over each dependency `D` and check `db.maybe_changed_since(D, Rv)`.
        * If no dependency has changed, update `verified_at` to current revision and **return** memoized value.
    * Mark memo as invalid by storing `None` in the verified-at.
3. Atomically check `sync_map` for an existing `Sync<Q>`:
    * If one exists, block on the thread within and return to step 2 after it completes:
        * If this results in a cycle, unwind as today.
    * If none exists, insert a new entry with current runtime-id.
4. Construct the new memo:
    * Push query onto the local stack and execute the query function:
        * If this query is found to be a cycle participant, execute recovery function.
    * Backdate result if it is equal to the old memo's value.
    * Allocate new memo.
5. Store results:
    * Store new memo into `memo_map[K]`.
    * Remove query from the `sync_map`.
6. **Return** newly constructed value._

**Verifying a dependency for a *derived* query** will work as follows. Verifying dependencies for other sorts of queries is unchanged.

1. Find internal index `K` by hashing key, as today.
    * Precise operation for this will depend on the concurrent hashmap implementation.
2. Load memo `M: Arc<Memo<Q>>` from `memo_map[K]` (if present):
    * If verified is `None`, then another thread has found this memo to be invalid; ignore it.
    * Else, let `Rv` be the "last verified revision".
    * If `Rv` is the current revision, **return** memoized value.
    * If last change to an input with durability `M.durability` was before `Rv`: 
        * Update `verified_at` to current revision and **return** memoized value.
    * Iterate over each dependency `D` and check `db.maybe_changed_since(D, Rv)`.
        * If no dependency has changed, update `verified_at` to current revision and **return** memoized value.
    * Mark memo as invalid by storing `None` in the verified-at.
3. Atomically check `sync_map` for an existing `Sync<Q>`:
    * If one exists, block on the thread within and return to step 2 after it completes:
        * If this results in a cycle, unwind as today.
    * If none exists, insert a new entry with current runtime-id.
4. Construct the new memo:
    * Push query onto the local stack and execute the query function:
        * If this query is found to be a cycle participant, execute recovery function.
    * Backdate result if it is equal to the old memo's value.
    * Allocate new memo.
5. Store results:
    * Store new memo into `memo_map[K]`.
    * Remove query from the `sync_map`.
6. **Return** true or false depending on whether memo was backdated.

### Phase 2 (future RFCs)

One of the goal of this RFC is to pave the way for a future extension in which we can support "fixed point cycle recovery". This is the form of cycle recovery used by chalk's recursive solver and it is appropriate for queries whose results repesent a search for answers.

## Frequently asked questions

### Why is the local query state important?

We don't actually use the local state in this RFC, that's true. It's purpose is to pave the way for fixed point cycle recovery.

### How do the synchronized / atomic operations compare after this RFC?

In order to perform a query, we used to acquire a read-lock to get the slot index and then a read-lock on the slot's contents to read its value or to verify its dependencies. The read-lock on the slot's contents was mandatory because it was possible.
