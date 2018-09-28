# salsa

*A generic framework for on-demand, incrementalized computation.*

## Obligatory warning

Very much a WORK IN PROGRESS at this point.

## Credits

This system is heavily inspired by adapton, glimmer, and rustc's query
system. So credit goes to Eduard-Mihai Burtescu, Matthew Hammer,
Yehuda Katz, and Michael Woerister.

## Goals

It tries to hit a few goals:

- We don't have to have a base crate that declares the "complete set of queries"
- Each module only has to know about the queries that it depends on and that it provides (but no others)
- Compiles to fast code, with no allocation, dynamic dispatch, etc on the "memoized hit" fast path
- Can recover from cycles gracefully (though I didn't really show that)

## How to use it

The way that it is meant to be used is roughly like this:

### Invoking a query

You will always be threading around a "query" context value. To invoke
a query `foo` on the value `key`, you do:

```rust
query.foo().of(key)
```

The syntax is intended to be extensible, so we can add other methods
besides `of` eventually (e.g., you might want methods that potentially
recover from a cycle, which `of` does not). We could change this to
`query.foo(key)`, which is what rustc does, with minimal hassle.

### Defining query traits

Each "major module" X will declare a trait with the queries it
provides. This trait should extend the traits for other modules that X
relies on. There will eventually be a "central context" that
implements all the traits for all the modules.

So, for example, a type checker module might declare:

```rust
crate trait TypeckQueryContext: crate::query::BaseQueryContext {
    query_prototype!(
        /// Find the fields of a struct.
        fn fields() for Fields
    );

    query_prototype!(
        /// Find the type of something.
        fn ty() for Ty
    );
}
```

Here, the trait basically says: "the final context must be provide
implementations of these two queries (`Fields` and `Ty`)". The macro
specifies a method name which is expected to be the "camel case"
version of the full query name (`fields`, `ty` respectively).

The `BaseQueryContext` trait is just the .. well .. basic query
context operations. In general, I would expect to see some other
modules instead, so something like:

```rust
crate trait TypeckQueryContext: HirQueryContext { .. }
```

where the `HirQueryContext` is a trait that defines the queries
related to HIR construction.

Note that these traits are not limited to containing queries: we can
basically add whatever methods we want that the "central context" must
provide (e.g., I expect to add a global interner in there).

### Defining query implementations

In addition to defining the trait with the queries that it exports,
the typeck module will also implement those queries. This is done most
easily by using the `query_definition!` macro. Here is an example
defining `Fields`:

```rust
query_definition! {
    /// Test documentation.
    crate Fields(_query: &impl TypeckQueryContext, _def_id: DefId) -> Arc<Vec<DefId>> {
        Arc::new(vec![])
    }
}
```

This is obviously a dummy implementation, but you get the idea.

### Defining query implementations the long-hand way

A query is really defined by a kind of "dummy" type that implements
the `Query` trait. This is the sort of code that macro above
generates:

```rust
#[derive(Default, Debug)]
crate struct Ty;

impl<QC> Query<QC> for Ty
where
    QC: TypeckQueryContext,
{
    type Key = DefId;
    type Value = Arc<Vec<DefId>>;
    type Storage = crate::query::storage::MemoizedStorage<QC, Self>;

    fn execute(query: &QC, key: DefId) -> Arc<Vec<DefId>> {
        query.ty().of(key)
    }
}
```

### Customizing query storage etc

Each query defines its storage. Right now I only sketched out one form
of storage -- memoized storage -- but eventualy I would expect to
permit a few options here. e.g., "immediate" queries, that just
*always* execute on demand, as well as queries that interact with an
incremental system.

