# Summary

- Introduce a user-visibile concept of `Durability`
- Adjusting the "durability" of an input can allow salsa to skip a lot of validation work
- Garbage collection -- particularly of interned values -- however becomes more complex
- Possible future expansion: automatic detection of more "durable" input values

# Motivation

## Making validation faster by optimizing for "durability"

Presently, salsa's validation logic requires traversing all
dependencies to check that they have not changed. This can sometimes
be quite costly in practice: rust-analyzer for example sometimes
spends as much as 90ms revalidating the results from a no-op
change. One option to improve this is simply optimization --
[salsa#176] for example reduces validation times significantly, and
there remains opportunity to do better still. However, even if we are
able to traverse the dependency graph more efficiently, it will still
be an O(n) process. It would be nice if we could do better.

[salsa#176]: https://github.com/salsa-rs/salsa/pull/176

One observation is that, in practice, there are often input values
that are known to change quite infrequently. For example, in
rust-analyzer, the standard library and crates downloaded from
crates.io are unlikely to change (though changes are possible; see
below). Similarly, the `Cargo.toml` file for a project changes
relatively infrequently compared to the sources. We say then that
these inputs are more **durable** -- that is, they change less frequently.

This RFC proposes a mechanism to take advantage of durability for
optimization purposes. Imagine that we have some query Q that depends
solely on the standard library. The idea is that we can track the last
revision R when the standard library was changed. Then, when
traversing dependencies, we can skip traversing the dependencies of Q
if it was last validated after the revision R. Put another way, we
only need to traverse the dependencies of Q when the standard library
changes -- which is unusual. If the standard library *does* change,
for example by user's tinkering with the internal sources, then yes we
walk the dependencies of Q to see if it is affected.

# User's guide

## The durability type

We add a new type `salsa::Durability` which has there associated constants:

```rust,ignore
#[derive(Copy, Clone, Debug, Ord)]
pub struct Durability(..);

impl Durability {
  // Values that change regularly, like the source to the current crate.
  pub const LOW: Durability;
  
  // Values that change infrequently, like Cargo.toml.
  pub const MEDIUM: Durability;

  // Values that are not expected to change, like sources from crates.io or the stdlib.
  pub const HIGH: Durability;
}
```

h## Specifying the durability of an input

When setting an input `foo`, one can now invoke a method
`set_foo_with_durability`, which takes a `Durability` as the final
argument:

```rust,ignore
// db.set_foo(key, value) is equivalent to:
db.set_foo_with_durability(key, value, Durability::LOW);

// This would indicate that `foo` is not expected to change: 
db.set_foo_with_durability(key, value, Durability::HIGH);
```

## Durability of interned values

Interned values are always considered `Durability::HIGH`. This makes
sense as many queries that only use high durability inputs will also
make use of interning internally. A consequence of this is that they
will not be garbage collected unless you use the specific patterns
recommended below.

## Synthetic writes

Finally, we add one new method, `synthetic_write(durability)`, 
available on the salsa runtime:

```rust,ignore
db.salsa_runtime().synthetic_write(Durability::HIGH)
```

As the name suggests, `synthetic_write` causes salsa to act *as
though* a write to an input of the given durability had taken
place. This can be used for benchmarking, but it's also important to
controlling what values get garbaged collected, as described below.

## Tracing and garbage collection

Durability affects garbage collection. The `SweepStrategy` struct is
modified as follows:

```rust,ignore
/// Sweeps values which may be outdated, but which have not
/// been verified since the start of the current collection.
/// These are typically memoized values from previous computations
/// that are no longer relevant.
pub fn sweep_outdated(self) -> SweepStrategy;

/// Sweeps values which have not been verified since the start 
/// of the current collection, even if they are known to be 
/// up to date. This can be used to collect "high durability" values
/// that are not *directly* used by the main query.
///
/// So, for example, imagine a main query `result` which relies
/// on another query `threshold` and (indirectly) on a `threshold_inner`:
///
/// ```
/// result(10) [durability: Low]
///    |
///    v
/// threshold(10) [durability: High]
///    |
///    v
/// threshold_inner(10)  [durability: High]
/// ```
///
/// If you modify a low durability input and then access `result`,
/// then `result(10)` and its *immediate* dependencies will 
/// be considered "verified". However, because `threshold(10)` 
/// has high durability and no high durability input was modified,
/// we will not verify *its* dependencies, so `threshold_inner` is not 
/// verified (but it is also not outdated).
///
/// Collecting unverified things would therefore collect `threshold_inner(10)`.
/// Collecting only *outdated* things (i.e., with `sweep_outdated`)
/// would collect nothing -- but this does mean that some high durability
/// queries that are no longer relevant to your main query may stick around.
/// 
/// To get the most precise garbage collection, do a synthetic write with
/// high durability -- this will force us to verify *all* values. You can then
/// sweep unverified values.
pub fn sweep_unverified(self) -> SweepStrategy;
```

# Reference guide

## Review: The need for GC to collect outdated values

In general, salsa's lazy validation scheme can lead to the accumulation
of garbage that is no longer needed. Consider a query like this one:

```rust,ignore
fn derived1(db: &impl Database, start: usize) {
  let middle = self.input(start);
  self.derived2(middle)
}
```

Now imagine that, on some particular run, we compute `derived1(22)`:

- `derived1(22)`
  - executes `input(22)`, which returns `44`
  - then executes `derived2(44)`
  
The end result of this execution will be a dependency graph
like:

```notrust
derived1(22) -> derived2(44)
  |
  v
input(22)
```

Now. imagine that the user modifies `input(22)` to have the value `45`.
The next time `derived1(22)` executes, it will load `input(22)` as before,
but then execute `derived2(45)`. This leaves us with a dependency
graph as follows:

```notrust
derived1(22) -> derived2(45)
  |
  v
input(22)       derived2(44)
```

Notice that we still see `derived2(44)` in the graph. This is because
we memoized the result in last round and then simply had no use for it
in this round. The role of GC is to collect "outdated" values like
this one.

###Review: Tracing and GC before durability

In the absence of durability, when you execute a query Q in some new
revision where Q has not previously executed, salsa must trace back
through all the queries that Q depends on to ensure that they are
still up to date. As each of Q's dependencies is validated, we mark it
to indicate that it has been checked in the current revision (and
thus, within a particular revision, we would never validate or trace a
particular query twice).

So, to continue our example, when we first executed `derived1(22)`
in revision R1, we might have had a graph like:


```notrust
derived1(22)   -> derived2(44)
[verified: R1]    [verified: R1]
  |
  v
input(22)
```

Now, after we modify `input(22)` and execute `derived1(22)` again, we 
would have a graph like:

```notrust
derived1(22)   -> derived2(45)
[verified: R2]    [verified: R2]
  |
  v
input(22)         derived2(44)
                  [verified: R1]
```

Note that `derived2(44)`, the outdated value, never had its "verified"
revision updated, because we never accessed it.

Salsa leverages this validation stamp to serve as the "marking" phase
of a simple mark-sweep garbage collector. The idea is that the sweep
method can collect any values that are "outdated" (whose "verified"
revision is less than the current revision).

The intended model is that one can do a "mark-sweep" style garbage
collection like so:

```rust,ignore
// Modify some input, triggering a new revision.
db.set_input(22, 45);

// The **mark** phase: execute the "main query", with the intention
// that we wish to retain all the memoized values needed to compute
// this main query, but discard anything else. For example, in an IDE
// context, this might be a "compute all errors" query.
db.derived1(22);

// The **sweep** phase: discard anything that was not traced during
// the mark phase.
db.sweep_all(...);
```

In the case of our example, when we execute `sweep_all`, it would
collect `derived2(44)`.

## Challenge: Durability lets us avoid tracing

This tracing model is affected by the move to durability. Now, if some
derived value has a high durability, we may skip tracing its
descendants altogether. This means that they would never be "verified"
-- that is, their "verified date" would never be updated.

This is why we modify the definition of "outdated" as follows:

- For a query value `Q` with durability `D`, let `R_lc` be the revision when
  values of durability `D` last changed. Let `R_v` be the revision when
  `Q` was last verified.
- `Q` is outdated if `R_v < R_lc`.
    - In other words, if `Q` may have changed since it was last verified.

## Collecting interned and untracked values

Most values can be collected whenever we like without influencing
correctness.  However, interned values and those with untracked
dependencies are an exception -- **they can only be collected when
outdated**.  This is because their values may not be reproducible --
in other words, re-executing an interning query (or one with untracked
dependencies, which can read arbitrary program state) twice in a row
may produce a different value. In the case of an interning query, for
example, we may wind up using a different integer than we did before.
If the query is outdated, this is not a problem: anything that
dependend on its result must also be outdated, and hence would be
re-executed and would observe the new value. But if the query is *not*
outdated, then we could get inconsistent result.s

# Alternatives and future work

## Rejected: Arbitrary durabilities

We considered permitting arbitrary "levels" of durability -- for
example, allowing the user to specify a number -- rather than offering
just three. Ultimately it seemed like that level of control wasn't
really necessary and that having just three levels would be sufficient
and simpler.

## Rejected: Durability lattices

We also considered permitting a "lattice" of durabilities -- e.g., to
mirror the crate DAG in rust-analyzer -- but this is tricky because
the lattice itself would be dependent on other inputs.

