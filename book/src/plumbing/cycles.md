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

When a cycle is detected, the current thread `T1` has full access to the query stacks that are participating in the cycle. Consider: naturally, `T1` has access to its own stack. There is also a path `T2 -> ... -> Tn -> T1` of blocked threads. Each of the blocked threads `T2 ..= Tn` will have moved their query stacks into the dependency graph, so those query stacks are available for inspection.

Using the available stacks, we can create a list of cycle participants `Q0 ... Qn` and store that into a `Cycle` struct. If none of the participants `Q0 ... Qn` have cycle recovery enabled, we panic with the `Cycle` struct, which will trigger all the queries on this thread to panic.

## Cycle recovery via fallback

If any of the cycle participants `Q0 ... Qn` has cycle recovery set, we recover from the cycle. To help explain how this works, we will use this example cycle which contains three threads. Beginning with the current query, the cycle participants are `QA3`, `QB2`, `QB3`, `QC2`, `QC3`, and `QA2`.

```
        The cyclic
        edge we have
        failed to add.
          :
   A      :    B         C
          :
   QA1    v    QB1       QC1
┌► QA2    ┌──► QB2   ┌─► QC2
│  QA3 ───┘    QB3 ──┘   QC3 ───┐
│                               │
└───────────────────────────────┘
```

Recovery works in phases:

* **Analyze:** As we enumerate the query participants, we collect their collective inputs (all queries invoked so far by any cycle participant) and the max changed-at and min duration. We then remove the cycle participants themselves from this list of inputs, leaving only the queries external to the cycle.
* **Mark**: For each query Q that is annotated with `#[salsa::cycle]`, we mark it and all of its successors on the same thread by setting its `cycle` flag to the `c: Cycle` we constructed earlier; we also reset its inputs to the collective inputs gathering during analysis. If those queries resume execution later, those marks will trigger them to immediately unwind and use cycle recovery, and the inputs will be used as the inputs to the recovery value.
    * Note that we mark *all* the successors of Q on the same thread, whether or not they have recovery set. We'll discuss later how this is important in the case where the active thread (A, here) doesn't have any recovery set.
* **Unblock**: Each blocked thread T that has a recovering query is forcibly reawoken; the outgoing edge from that thread to its successor in the cycle is removed. Its condvar is signalled with a `WaitResult::Cycle(c)`. When the thread reawakens, it will see that and start unwinding with the cycle `c`.
* **Handle the current thread:** Finally, we have to choose how to have the current thread proceed. If the current thread includes any cycle with recovery information, then we can begin unwinding. Otherwise, the current thread simply continues as if there had been no cycle, and so the cyclic edge is added to the graph and the current thread blocks. This is possible because some other thread had recovery information and therefore has been awoken.

Let's walk through the process with a few examples.

### Example 1: Recovery on the detecting thread

Consider the case where only the query QA2 has recovery set. It and QA3 will be marked with their `cycle` flag set to `c: Cycle`. Threads B and C will not be unblocked, as they do not have any cycle recovery nodes. The current thread (Thread A) will initiate unwinding with the cycle `c` as the value. Unwinding will pass through QA3 and be caught by QA2. QA2 will substitute the recovery value and return normally. QA1 and QC3 will then complete normally and so forth, on up until all queries have completed.

### Example 2: Recovery in two queries on the detecting thread

Consider the case where both query QA2 and QA3 have recovery set. It proceeds the same Example 1 until the the current initiates unwinding, as described in Example 1. When QA3 receives the cycle, it stores its recovery value and completes normally. QA2 then adds QA3 as an input dependency: at that point, QA2 observes that it too has the cycle mark set, and so it initiates unwinding. The rest of QA2 therefore never executes. This unwinding is caught by QA2's entry point and it stores the recovery value and returns normally. QA1 and QC3 then continue normally, as they have not had their `cycle` flag set.

### Example 3: Recovery on another thread

Now consider the case where only the query QB2 has recovery set. It and QB3 will be marked with the cycle `c: Cycle` and thread B will be unblocked; the edge `QB3 -> QC2` will be removed from the dependency graph. Thread A will then add an edge `QA3 -> QB2` and block on thread B. At that point, thread A releases the lock on the dependency graph, and so thread B is re-awoken. It observes the `WaitResult::Cycle` and initiates unwinding. Unwinding proceeds through QB3 and into QB2, which recovers. QB1 is then able to execute normally, as is QA3, and execution proceeds from there.

### Example 4: Recovery on all queries

Now consider the case where all the queries have recovery set. In that case, they are all marked with the cycle, and all the cross-thread edges are removed from the graph. Each thread will independently awaken and initiate unwinding. Each query will recover.
