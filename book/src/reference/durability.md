# Durability

"Durability" is an optimization that can greatly improve the performance of your salsa programs.
Durability specifies the probability that an input's value will change.
The default is "low durability".
But when you set the value of an input, you can manually specify a higher durability,
typically `Durability::HIGH`.
Salsa tracks when tracked functions only consume values of high durability
and, if no high durability input has changed, it can skip traversing their
dependencies.

Typically "high durability" values are things like data read from the standard library
or other inputs that aren't actively being edited by the end user.