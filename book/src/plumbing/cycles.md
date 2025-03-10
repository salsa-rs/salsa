# Cycles

## Cross-thread blocking

The interface for blocking across threads now works as follows:

* When one thread `T1` wishes to block on a query `Q` being executed by another thread `T2`, it invokes `Runtime::try_block_on`. This will check for cycles. Assuming no cycle is detected, it will block `T1` until `T2` has completed with `Q`. At that point, `T1` reawakens. However, we don't know the result of executing `Q`, so `T1` now has to "retry". Typically, this will result in successfully reading the cached value.
* While `T1` is blocking, the runtime moves its query stack (a `Vec`) into the shared dependency graph data structure. When `T1` reawakens, it recovers ownership of its query stack before returning from `try_block_on`.

## Cycle detection

When a thread `T1` attempts to execute a query `Q`, it will try to load the value for `Q` from the memoization tables. If it finds an `InProgress` marker, that indicates that `Q` is currently being computed. This indicates a potential cycle. `T1` will then try to block on the query `Q`:

* If `Q` is also being computed by `T1`, then there is a cycle.
* Otherwise, if `Q` is being computed by some other thread `T2`, we have to check whether `T2` is (transitively) blocked on `T1`. If so, there is a cycle.

These two cases are handled internally by the `Runtime::try_block_on` function. Detecting the intra-thread cycle case is easy; to detect cross-thread cycles, the runtime maintains a dependency DAG between threads (identified by `RuntimeId`). Before adding an edge `T1 -> T2` (i.e., `T1` is blocked waiting for `T2`) into the DAG, it checks whether a path exists from `T2` to `T1`. If so, we have a cycle and the edge cannot be added (then the DAG would not longer be acyclic).
