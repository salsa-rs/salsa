# Untracked dependency

An *untracked dependency* is an indication that the result of a [derived query] depends on something not visible to the salsa database. Untracked dependencies are created by invoking [`report_untracked_read`](https://docs.rs/salsa/0.16.1/salsa/struct.Runtime.html#method.report_untracked_read) or [`report_synthetic_read`](https://docs.rs/salsa/0.16.1/salsa/struct.Runtime.html#method.report_synthetic_read). When an untracked dependency is present, [derived queries] are always re-executed if the durability check fails (see the description of the [fetch operation] for more details).

[derived query]: ./derived_query.md
[derived queries]: ./derived_query.md
[fetch operation]: ../fetch.md#derived-queries
