# Verified

A [memo] is *verified* in a revision R if we have checked that its value is still up-to-date (i.e., if we were to reexecute the [query function], we are guaranteed to get the same result). Each memo tracks the revision in which it was last verified to avoid repeatedly checking whether dependencies have changed during the [fetch] and [maybe changed after] operations.

[query function]: ./query_function.md
[fetch]: ../fetch.md
[maybe changed after]: ../maybe_changed_after.md
[memo]: ./memo.md