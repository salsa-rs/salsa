# The "red-green" algorithm

This page explains the basic Salsa incremental algorithm.
The algorithm is called the "red-green" algorithm, which is where the name Salsa comes from.

### Database revisions

The Salsa database always tracks a single **revision**. Each time you set an input, the revision is incremented. So we start in revision `R1`, but when a `set` method is called, we will go to `R2`, then `R3`, and so on. For each input, we also track the revision in which it was last changed.

### Basic rule: when inputs change, re-execute!

When you invoke a tracked function, in addition to storing the value that was returned, we also track what _other_ tracked functions it depends on, and the revisions when their value last changed. When you invoke the function again, if the database is in a new revision, then we check whether any of the inputs to this function have changed in that new revision. If not, we can just return our cached value. But if the inputs _have_ changed (or may have changed), we will re-execute the function to find the most up-to-date answer.

Here is a simple example, where the `parse_module` function invokes the `module_text` function:

```rust
#[salsa::tracked]
fn parse_module(db: &dyn Db, module: Module) -> Ast {
    let module_text: &String = module_text(db, module);
    Ast::parse_text(module_text)
}

#[salsa::tracked(return_ref)]
fn module_text(db: &dyn Db, module: Module) -> String {
    panic!("text for module `{module:?}` not set")
}
```

If we invoke `parse_module` twice, but change the module text in between, then we will have to re-execute `parse_module`:

```rust
module_text::set(
    db,
    module,
    "fn foo() { }".to_string(),
);
parse_module(db, module); // executes

// ...some time later...

module_text::set(
    db,
    module,
    "fn foo() { /* add a comment */ }".to_string(),
);
parse_module(db, module); // executes again!
```

### Backdating: sometimes we can be smarter

Often, though, tracked functions don't depend directly on the inputs. Instead, they'll depend on some other tracked function. For example, perhaps we have a `type_check` function that reads the AST:

```rust
#[salsa::tracked]
fn type_check(db: &dyn Db, module: Module) {
    let ast = parse_module(db, module);
    ...
}
```

If the module text is changed, we saw that we have to re-execute `parse_module`, but there are many changes to the source text that still produce the same AST. For example, suppose we simply add a comment? In that case, if `type_check` is called again, we will:

- First re-execute `parse_module`, since its input changed.
- We will then compare the resulting AST. If it's the same as last time, we can _backdate_ the result, meaning that we say that, even though the inputs changed, the output didn't.

## Durability: an optimization

As an optimization, Salsa includes the concept of **durability**, which is the notion of how often some piece of tracked data changes. 

For example, when compiling a Rust program, you might mark the inputs from crates.io as _high durability_ inputs, since they are unlikely to change. The current workspace could be marked as _low durability_, since changes to it are happening all the time.

When you set the value of a tracked function, you can also set it with a given _durability_:

```rust
module_text::set_with_durability(
    db,
    module,
    "fn foo() { }".to_string(),
    salsa::Durability::HIGH
);
```

For each durability, we track the revision in which _some input_ with that durability changed. If a tracked function depends (transitively) only on high durability inputs, and you change a low durability input, then we can very easily determine that the tracked function result is still valid, avoiding the need to traverse the input edges one by one.

