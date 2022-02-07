# Parallel friendly caching

## Metadata

* Author: nikomatsakis
* Date: 2021-05-29
* Introduced in: (please update once you open your PR)

## Summary

* Rework query storage to be based on concurrent hashmaps instead of slots with read-write locked state.

## Motivation

Two-fold:

* Simpler, cleaner, and hopefully faster algorithm.
* Enables some future developments that are not part of this RFC:
    * Derived queries whose keys are known to be integers.
    * Fixed point cycles so that salsa and chalk can be deeply integrated.
    * Non-synchronized queries that potentially execute on many threads in parallel (required for fixed point cycles, but potentially valuable in their own right).

## User's guide

No user visible changes.

## Reference guide

### Background: Current structure

Before this RFC, the **overall structure** of derived queries is as follows:

* Each derived query has a `DerivedStorage<Q>` (stored in the database) that contains:
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

[maybe changed after]: ../plumbing/maybe_changed_after.md
[fetch]: ../plumbing/fetch.md

**Fetching the value for a query** currently works as follows:

* Acquire the read lock on the (indexable) `slot_map` and hash key to find the slot.
    * If no slot exists, acquire write lock and insert.
* Acquire the slot's internal lock to perform the [fetch] operation.

**Verifying a dependency** uses a scheme introduced in [RFC #6](./RFC0006-Dynamic-Databases.md). Each dependency is represented as a `DatabaseKeyIndex` which contains three indices (group, query, and key). The group and query indices are used to find the query storage via `match` statements and then the next operation depends on the query type:

* Acquire the read lock on the (indexable) `slot_map` and use key index to load the slot. Read lock is released afterwards.
* Acquire the slot's internal lock to perform the [maybe changed after] operation.

### New structure (introduced by this RFC)

The **overall structure** of derived queries after this RFC is as follows:

* Each derived query has a `DerivedStorage<Q>` (stored in the database) that contains:
    * a set of concurrent hashmaps:
        * `key_map`: maps from `Q::Key` to an internal key index `K`
        * `memo_map`: maps from `K` to cached memo `ArcSwap<Memo<Q>>`
        * `sync_map`: maps from `K` to a `Sync<Q>` synchronization value
    * lru set
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

**Fetching the value for a *derived* query** will work as follows:

1. Find internal index `K` by hashing key, as today.
    * Precise operation for this will depend on the concurrent hashmap implementation.
2. Load memo `M: Arc<Memo<Q>>` from `memo_map[K]` (if present):
    * If verified is `None`, then another thread has found this memo to be invalid; ignore it.
    * Else, let `Rv` be the "last verified revision".
    * If `Rv` is the current revision, or last change to an input with durability `M.durability` was before `Rv`:
        * Update "last verified revision" and **return** memoized value.
3. Atomically check `sync_map` for an existing `Sync<Q>`:
    * If one exists, block on the thread within and return to step 2 after it completes:
        * If this results in a cycle, unwind as today.
    * If none exists, insert a new entry with current runtime-id.
4. Check dependencies deeply
    * Iterate over each dependency `D` and check `db.maybe_changed_after(D, Rv)`.
        * If no dependency has changed, update `verified_at` to current revision and **return** memoized value.
    * Mark memo as invalid by storing `None` in the verified-at.
5. Construct the new memo:
    * Push query onto the local stack and execute the query function:
        * If this query is found to be a cycle participant, execute recovery function.
    * Backdate result if it is equal to the old memo's value.
    * Allocate new memo.
6. Store results:
    * Store new memo into `memo_map[K]`.
    * Remove query from the `sync_map`.
7. **Return** newly constructed value._

**Verifying a dependency for a *derived* query** will work as follows:

1. Find internal index `K` by hashing key, as today.
    * Precise operation for this will depend on the concurrent hashmap implementation.
2. Load memo `M: Arc<Memo<Q>>` from `memo_map[K]` (if present):
    * If verified is `None`, then another thread has found this memo to be invalid; ignore it.
    * Else, let `Rv` be the "last verified revision".
    * If `Rv` is the current revision, **return** true or false depending on whether changed-at from memo.
    * If last change to an input with durability `M.durability` was before `Rv`: 
        * Update `verified_at` to current revision and **return** memoized value.
    * Iterate over each dependency `D` and check `db.maybe_changed_after(D, Rv)`.
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

## Frequently asked questions

### Why use `ArcSwap`?

It's a relatively minor implementation detail, but the code in this PR uses `ArcSwap` to store the values in the memo-map. In the case of a cache hit or other transient operations, this allows us to read from the arc while avoiding a full increment of the ref count. It adds a small bit of complexity because we have to be careful to do a full load before any recursive operations, since arc-swap only gives a fixed number of "guards" per thread before falling back to more expensive loads.

### Do we really need `maybe_changed_after` *and* `fetch`?

Yes, we do. "maybe changed after" is very similar to "fetch", but it doesn't require that we have a memoized value. This is important for LRU.

### The LRU map in the code is just a big lock!

That's not a question. But it's true, I simplified the LRU code to just use a mutex. My assumption is that there are relatively few LRU-ified queries, and their values are relatively expensive to compute, so this is ok. If we find it's a bottleneck, though, I believe we could improve it by using a similar "zone scheme" to what we use now. We would add a `lru_index` to the `Memo` so that we can easily check if the memo is in the "green zone" when reading (if so, no updates are needed). The complexity there is that when we produce a replacement memo, we have to install it and swap the index. Thinking about that made my brain hurt a little so I decided to just take the simple option for now.

### How do the synchronized / atomic operations compare after this RFC?

After this RFC, to perform a read, in the best case:

* We do one "dashmap get" to map key to key index.
* We do another "dashmap get" from key index to memo.
* We do an "arcswap load" to get the memo.
* We do an "atomiccell read" to load the current revision or durability information.

dashmap is implemented with a striped set of read-write locks, so this is roughly the same (two read locks) as before this RFC. However:

* We no longer do any atomic ref count increments.
* It is theoretically possible to replace dashmap with something that doesn't use locks.
* The first dashmap get should be removable, if we know that the key is a 32 bit integer.
    * I plan to propose this in a future RFC.

### Yeah yeah, show me some benchmarks!

I didn't run any. I'll get on that.
