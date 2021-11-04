# Memo

A *memo* stores information about the last time that a [query function] for some [query] Q was executed:

* Typically, it contains the value that was returned from that function, so that we don't have to execute it again.
    * However, this is not always true: some queries don't cache their result values, and values can also be dropped as a result of [LRU] collection. In those cases, the memo just stores [dependency] information, which can still be useful to determine if other queries that have Q as a [dependency] may have changed.
* The revision in which the memo last [verified].
* The [changed at] revision in which the memo's value last changed. (Note that it may be [backdated].)
* The minimum durability of the memo's [dependencies].
* The complete set of [dependencies], if available, or a marker that the memo has an [untracked dependency].

[revision]: ./revision.md
[backdated]: ./backdate.md
[dependencies]: ./dependency.md
[dependency]: ./dependency.md
[untracked dependency]: ./untracked.md
[verified]: ./verified.md
[query]: ./query.md
[query function]: ./query_function.md
[changed at]: ./changed_at.md
[LRU]: ./LRU.md