# Maybe changed after

```rust,no_run,noplayground
{{#include ../../../src/plumbing.rs:maybe_changed_after}}
```

The `maybe_changed_after` operation computes whether a query's value *may have changed* **after** the given revision. In other words, `Q.maybe_change_since(R)` is true if the value of the query `Q` may have changed in the revisions `(R+1)..R_now`, where `R_now` is the current revision. Note that it doesn't make sense to ask `maybe_changed_after(R_now)`.

## Input queries

Input queries are set explicitly by the user. `maybe_changed_after` can therefore just check when the value was last set and compare.

## Interned queries

## Derived queries

The logic for derived queries is more complex. We summarize the high-level ideas here, but you may find the [flowchart](./derived_flowchart.md) useful to dig deeper. The [terminology](./terminology.md) section may also be useful; in some cases, we link to that section on the first usage of a word.

* If an existing [memo] is found, then we check if the memo was [verified] in the current [revision]. If so, we can compare its [changed at] revision and return true or false appropriately.
* Otherwise, we must check whether [dependencies] have been modified:
    * Let R be the revision in which the memo was last verified; we wish to know if any of the dependencies have changed since revision R.
    * First, we check the [durability]. For each memo, we track the minimum durability of the memo's dependencies. If the memo has durability D, and there have been no changes to an input with durability D since the last time the memo was verified, then we can consider the memo verified without any further work.
    * If the durability check is not sufficient, then we must check the dependencies individually. For this, we iterate over each dependency D and invoke the [maybe changed after](./maybe_changed_after.md) operation to check whether D has changed since the revision R.
    * If no dependency was modified:
        * We can mark the memo as verified and use its [changed at] revision to return true or false.
* Assuming dependencies have been modified:
    * Then we execute the user's query function (same as in [fetch]), which potentially [backdates] the resulting value.
    * Compare the [changed at] revision in the resulting memo and return true or false.

[changed at]: ./terminology/changed_at.md
[durability]: ./terminology/durability.md
[backdate]: ./terminology/backdate.md
[backdates]: ./terminology/backdate.md
[dependency]: ./terminology/dependency.md
[dependencies]: ./terminology/dependency.md
[memo]: ./terminology/memo.md
[revision]: ./terminology/revision.md
[verified]: ./terminology/verified.md
[fetch]: ./fetch.md
[LRU]: ./terminology/LRU.md