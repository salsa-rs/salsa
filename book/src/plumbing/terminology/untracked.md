# Untracked dependency

An *untracked dependency* is an indication that the result of a [derived query] depends on something not visible to the salsa database. Untracked dependencies are created by invoking [`Database::report_untracked_read`](https://docs.rs/salsa/latest/salsa/trait.Database.html#method.report_untracked_read). When an untracked dependency is present, [derived queries] are always re-executed in a later revision (see the description of the [fetch operation] for more details).

[derived query]: ./derived_query.md
[derived queries]: ./derived_query.md
[fetch operation]: ../fetch.md#derived-queries
