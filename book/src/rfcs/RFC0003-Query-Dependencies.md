# Summary

Allow to specify a dependency on a query group without making it a super trait.

# Motivation

Currently, there's only one way to express that queries from group `A` can use
another group `B`: namely, `B` can be a super-trait of `A`:

```rust
#[salsa::query_group(AStorage)]
trait A: B {

}
```

This approach works and allows one to express complex dependencies. However,
this approach falls down when one wants to make a dependency a private
implementation detail: Clients with `db: &impl A` can freely call `B` methods on
the `db`.

This is a bad situation from software engineering point of view: if everything
is accessible, it's hard to make distinction between public API and private
implementation details. In the context of salsa the situation is even worse,
because it breaks "firewall" pattern. It's customary to wrap low-level
frequently-changing or volatile queries into higher-level queries which produce
stable results and contain invalidation. In the current salsa, however, it's
very easy to accidentally call a low-level volatile query instead of a wrapper,
introducing and undesired dependency.

# User's guide

To specify query dependencies, a `requires` attribute should be used:

```rust
#[salsa::query_group(SymbolsDatabaseStorage)]
#[salsa::requires(SyntaxDatabase)]
#[salsa::requires(EnvDatabase)]
pub trait SymbolsDatabase {
    fn get_symbol_by_name(&self, name: String) -> Symbol;
}
```

The argument of `requires` is a path to a trait. The traits from all `requires`
attributes are available when implementing the query:

```rust
fn get_symbol_by_name(
    db: &(impl SymbolsDatabase + SyntaxDatabase + EnvDatabase),
    name: String,
) -> Symbol {
    // ...
}
```

However, these traits are **not** available without explicit bounds:

```rust
fn fuzzy_find_symbol(db: &impl SymbolsDatabase, name: String) {
    // Can't accidentally call methods of the `SyntaxDatabase`
}
```

Note that, while the RFC does not propose to add per-query dependencies, query
implementation can voluntarily specify only a subset of traits from `requires`
attribute:

```rust
fn get_symbol_by_name(
    // Purposefully don't depend on EnvDatabase
    db: &(impl SymbolsDatabase + SyntaxDatabase),
    name: String,
) -> Symbol {
    // ...
}
```

# Reference guide

The implementation is straightforward and consists of adding traits from
`requires` attributes to various `where` bounds. For example, we would generate
the following blanket for above example:

```rust
impl<T> SymbolsDatabase for T
where
    T: SyntaxDatabase + EnvDatabase,
    T: salsa::plumbing::HasQueryGroup<SymbolsDatabaseStorage>
{
    ...
}
```

# Alternatives and future work

The semantics of `requires` closely resembles `where`, so we could imagine a
syntax based on magical where clauses:

```rust
#[salsa::query_group(SymbolsDatabaseStorage)]
pub trait SymbolsDatabase
    where ???: SyntaxDatabase + EnvDatabase
{
    fn get_symbol_by_name(&self, name: String) -> Symbol;
}
```

However, it's not obvious what should stand for `???`. `Self` won't be ideal,
because supertraits are a sugar for bounds on `Self`, and we deliberately want
different semantics. Perhaps picking a magical identifier like `DB` would work
though?

One potential future development here is per-query-function bounds, but they can
already be simulated by voluntarily requiring less bounds in the implementation
function.

Another direction for future work is privacy: because traits from `requires`
clause are not a part of public interface, in theory it should be possible to
restrict their visibility. In practice, this still hits public-in-private lint,
at least with a trivial implementation.

