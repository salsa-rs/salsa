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
    * In general, it would be simpler if we could move cycle handling to *per-thread*, although there are limits to how much we can do this.
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

## Frequently asked questions

Use this section to add in design notes, downsides, rejected approaches, or other considerations.

