# Derived query

A *derived query* is a [query] whose value is defined by the result of a user-provided [query function]. That function is executed to get the result of the query. Unlike [input queries], the result of a derived queries can always be recomputed whenever needed simply by re-executing the function.

[query]: ./query.md
[query function]: ./query_function.md
[input queries]: ./input_query.md