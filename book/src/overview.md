# Salsa overview

{{#include caveat.md}}

This page contains a brief overview of the pieces of a salsa program. For a more detailed look, check out the [tutorial](./tutorial.md), which walks through the creation of an entire project end-to-end.

## Database

Every salsa program has an omnipresent _database_, which stores all the data across revisions. As you change the inputs to your program, we will consult this database to see if there are old computations that can be reused. The database is also used to implement interning and other convenient features.

## Memoized functions

The most basic concept in salsa is a **memoized function**. When you mark a function as memoized, that indicates that you would like to store its value in the database:

```rust
#[salsa::memoized]
fn parse_module(db: &dyn Db, module: Module) -> Ast {
    ...
}
```

When you call a memoized function, we first check if we can find the answer in the database. In that case, we return a clone of the saved answer instead of executing the function twice.

Sometimes you have memoized functions whose return type might be expensive to clone. In that case, you can mark the memoized function as `return_ref`. When you call a `return_ref` function, we will return a reference to the memoized result in the database:

```rust
#[salsa::memoized(return_ref)]
fn module_text(db: &dyn Db, module: Module) -> &String {
    ...
}
```

## Inputs and revisions

Each memoized function has an associated `set` method that can be used to set a return value explicitly. Memoized functions whose values are explicitly set are called _inputs_.

```rust
fn load_module_source(db: &mut dyn Db, module: Module) {
    let source: String = load_source_text();
    module_text::set(db, module, source);
    //           ^^^ set function!
}
```

Often, inputs don't have a function body, but simply panic in the case that they are not set explicitly, but this is not required. For example, the `module_text` function returns the raw bytes for a module. This is likely not something we can compute from "inside" the system, so the definition might just panic:

```rust
#[salsa::memoized(return_ref)]
fn module_text(db: &dyn Db, module: Module) -> String {
    panic!("text for module `{module:?}` not set")
}
```

Each time you invoke `set`, you begin a new **revision** of the database. Each memoized result in the database tracks the revision in which it was computed; invoking `set` may invalidate memoized results, causing functions to be re-executed (see the reference for [more details on how salsa decides when a memoized result is outdated](./reference/algorithm.md)).

## Entity values

Entity structs are special structs whose fields are versioned and stored in the database. For example, the `Module` type that we have been passing around could potentially be declared as an entity:

```rust
#[salsa::entity]
struct Module {
    #[return_ref]
    path: String,
}
```

A new module could be created with the `new` method:

```rust
let m: Module = Module::new(db, "some_path".to_string());
```

Despite the struct declaration above, the actual `Module` struct is just a newtyped integer, guaranteed to be unique within this database revision. You can access fields via accessors like `m.path(db)` (the `#[return_ref]` attribute here indicates that a `path` returns an `&String`, and not a cloned `String`).

## Interned values

In addition to entities, you can also declare _interned structs_ (and enums). Interned structs take arbitrary data and replace it with an integer. Unlike an entity, where each call to `new` returns a fresh integer, interning the same data twice gives back the same integer.

A common use for interning is to intern strings:

```rust
#[salsa::interned]
struct Word {
    #[return_ref]
    text: String
}
```

Interning the same value twice gives the same integer, so in this code...

```rust
let w1 = Word::new(db, "foo".to_string());
let w2 = Word::new(db, "foo".to_string());
```

...we know that `w1 == w2`.

## Accumulators

The final salsa concept are **accumulators**. Accumulators are a way to report errors or other "side channel" information that is separate from the main return value of your function.

To create an accumulator, you declare a type as an _accumulator_:

```rust
#[salsa::accumulator]
pub struct Diagnostics(String);
```

It must be a newtype of something, like `String`. Now, during a memoized function's execution, you can push those values:

```rust
Diagnostics::push(db, "some_string".to_string())
```

Then later, from outside the execution, you can ask for the set of diagnostics that were accumulated by some particular memoized function. For example, imagine that we have a type-checker and, during type-checking, it reports some diagnostics:

```rust
#[salsa::memoized]
fn type_check(db: &dyn Db, module: Module) {
    // ...
    Diagnostics::push(db, "some error message".to_string())
    // ...
}
```

we can then later invoke the associated `accumulated` function to get all the `String` values that were pushed:

```rust
let v: Vec<String> = type_check::accumulated::<Diagnostics>(db);
```
