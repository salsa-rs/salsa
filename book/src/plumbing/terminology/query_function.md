# Query function

The *query function* is the user-provided function that we execute to compute the value of a [derived query]. Salsa assumed that all query functions are a 'pure' function of their [dependencies] unless the user reports an [untracked read]. Salsa always assumes that functions have no important side-effects (i.e., that they don't send messages over the network whose results you wish to observe) and thus that it doesn't have to re-execute functions unless it needs their return value.

[derived query]: ./derived_query.md
[dependencies]: ./dependency.md
[untracked read]: ./untracked.md