# Backdate

*Backdating* is when we mark a value that was computed in revision R as having last changed in some earlier revision. This is done when we have an older [memo] M and we can compare the two values to see that, while the [dependencies] to M may have changed, the result of the [query function] did not.

[memo]: ./memo.md
[dependencies]: ./dependency.md
[query function]: ./query_function.md