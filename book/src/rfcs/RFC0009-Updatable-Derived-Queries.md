# Updatable derived queries

## Metadata

* Author: 1tgr
* Date: 2021-06-17
* Introduced in: https://github.com/salsa-rs/salsa/pull/269

## Summary

- Allow a query function to reuse the latest value on the same query

## Motivation

Derived queries in Salsa are implemented in terms of a pure function over a key and some other
queries ('dependencies'). The value returned by the query is expected to depend only on the query
key and the values of the dependencies.

However, in performance terms, it can be helpful if the query's function can also see the most
recent value calculated for the same query. This allows it to perform an incremental update to this
existing value instead of constructing a new value from scratch.

As a performance optimisation, this technique generally provides a benefit by allowing the query
function to reuse an existing memory allocation ("mine it for spare parts"). Some examples:

- Incrementally re-parsing a source file after a diff is applied
- Trait matching, chalk-style, where we can generally keep the tables in between queries but if the
  set of impls etc changes we may want to throw them away
- Large values that want to reuse the allocation in a previous `Vec`, `String` or `HashMap`
- Any persistent data structure

## User's guide

Conventionally, you define a Salsa query in terms of a single function on a trait:

```rust
#[salsa::query_group]
trait MyDatabase {
    fn formatted_value(&self, key: u32) -> String;
}

fn formatted_value(db: &dyn MyDatabase, key: u32) -> String {
    db.some_input(key).to_string()
}
```

When you apply the `#[salsa::update]` attribute to the function, Salsa looks for a second function,
which it calls to update an existing value in response to changes in the query's dependencies. By
default the update function has the word `update_` prepended to its name. Salsa passes an
`&mut Value` reference to the update function, and it expects the update function to return
`ValueChanged::True` unless the value is unchanged.

```rust
#[salsa::query_group]
trait MyDatabase {
    #[salsa::update]
    fn formatted_value(&self, key: u32) -> String;
}

fn formatted_value(db: &dyn MyDatabase, key: u32) -> String {
    // Our query is being called for the first time and it has no value already cached.
    // Return a new String.
    db.some_input(key).to_string()
}

fn update_formatted_value(db: &dyn MyDatabase, key: u32, value: &mut String) -> salsa::ValueChanged {
    // Write a new value into the existing String.
  
    // 1. Clear the existing String while keeping the memory allocation intact.
    value.clear();

    // 2. Without allocating new memory, write the new value into the String.
    use std::fmt::Write;
    let _ = write!(value, "{}", db.some_input(key));

    // 3. We assume the value in the String has changed.
    salsa::ValueChanged::True
}
```

## Reference guide

### `QueryFunction`
The `QueryFunction` trait has functions `init` and `update`, corresponding to the two functions
implemented by the user. The `update` function is responsible for making a best-effort determination
of whether it changed the value: it has access to the `MP: MemoizationPolicy` generic parameter and
returns `Value_changed`.

### Query execution
In the case where no cached value exists (either the query is being called for the first time, or
the query is not memoized), `Slot::read_upgrade` calls `QueryFunction::init`.

Where a cached value does exist, `Slot::read_upgrade` does the following:
1. `Option::take` on the cached value, to move the cached value to a local variable within
   `read_upgrade` and set the cached value to `None` while the query is being executed.
2. Call `QueryFunction::update` with a mutable reference to the value. `update` modifies the value
   in place through the reference and returns `ValueChanged`.
3. When `ValueChanged == True`, resets `memo.revisions.changed_at` to the revision it had before the
   update
4. Moves the value back to the cache

Query functions that do not use the `#[salsa::update]` attribute implement `init` and `update` in
terms of a single `execute` function:

```
fn init(db, key) {
    execute(db, key)
}

fn update<MP>(db, key, value) {
    let old_value = mem::replace(value, execute(db, key));
    MP::memoized_value_changed(&old_value, value)
}
```

The user can specify the function name in `#[salsa::update(update_formatted_value)]` to override
the default naming convention.

## Frequently asked questions

### This seems complicated. Can the functions be simplified?

In summary: not without reducing the user's abilty to optimise.

Consider this feature as implemented in terms of a single function that takes `Option<Self::Value>`
and returns `Self::Value`:

```rust
fn formatted_value(db: &dyn MyDatabase, value: Option<String>, key: u32) -> String {
    let mut value = if let Some(value) = value {
        // The update case
        value.clear();
        value
    } else {
        // The init case
        String::new()
    };

    use std::fmt::Write;
    let _ = write!(value, "{}", db.some_input(key));
    value
}
```

To call this function, `Slot::read_upgrade` clones the cached value to pass it to the execute
function. Although cloning has some runtime cost, we could assume it is minimal - after all,
there is already a call to `value.clone()` in the case where the query inputs are unchanged.

However, consider this optimisation to the `formatted_value` function, which replaces `String` with
`Arc<String>` in the hope of reducing the cost of cloning values:

```rust
fn formatted_value(db: &dyn MyDatabase, value: Option<Arc<String>>, key: u32) -> Arc<String> {
    // 1. Initialize to a blank string (inside Arc) on the first call.
    let mut value_arc = value.unwrap_or_default();
    
    // 2. Call Arc::make_mut, which avoids a clone provided nobody else holds an Arc on the same data. 
    let mut value_mut = Arc::make_mut(&mut value_arc);

    // 3. Write the value
    use std::fmt::Write;
    value_mut.clear();
    let _ = write!(value_mut, "{}", db.some_input(key));
    
    // 4. Return an Arc. Hopefully Arc::make_mut took the happy path and this is the same Arc that
    // was passed to the function.
    value_arc
}
```

If the `Arc<String>` is also held within the Salsa cache while `formatted_value` is called then
`Arc::make_mut` will never take the happy path: it will always need to make a clone into a new
allocation.

Salsa can enable the `Arc::make_mut` optimisation provided it moves the value out of the cache
before calling the update function.

The side effect of this is that the old and new values are not necessarily observable at the same
time, which means Salsa cannot directly compare them via the `MemoizationPolicy`; responsibility
for this comparison moves to the update function itself. Besides, an update function that makes an
incremental update to a data structure is probably in a better position to determine a
`value_changed` flag, compared with `Eq::eq` which must look at two versions of a data structure
without knowledge of the changes applied.

### How does this interact with panics and cancellation?

Because the cached value is `take()`n from the cache before calling the update function, a panic
in the update function leaves the cache empty. Since the update function modifies the value in
place, we can't reasonably restore the old value in the event of a panic - we would instead need to
incrementally undo any changes already made to the value.

Under this proposal we leave the cache empty in the event of a panic and recalculate it from scratch
(using the init function) the next time it is needed. This is a change to Salsa's previous behaviour
whereby the old value is preserved and the cache is not invalidated.

### What happens if `update` returns the wrong `value_changed` flag?

It's not a logic error for `update` to always return `ValueChanged::True`. The effect of this is
that downstream queries are re-run even though this query's value has not changed.

Returning `ValueChanged::False` from `update`, when the value has in fact changed, causes downstream
values to remain stale. This gives unexpected results but is not unsound. It's equivalent to a buggy
`PartialEq::eq` or `Hash::hash`.

### Is there a chance that `update` forgets to update and stale data remains in the value?

Indeed the `init`/`update` pair of functions is less pure than a single `execute`. Typically an
`update` function wants to reuse a data structure's memory allocation but not the data within it,
in which case the first line of the `update` function is `value.clear();`.

Without this update mechanism in the framework, a query function that was determined to re-use a
previous result would need to use a less pure approach, such as global variables, or an object
pool accessed via a Salsa input.

### What if `init` and `update` functions want to share logic?

In the case of a complex query, the majority of the `init` and `update` logic will be the same.
Under this proposal, code reuse would be addressed by them calling a third function, or having
`init` construct an empty value and delegate to `update`.

An alternative approach would be for the framework to call `init` once to construct an empty value,
then unconditionally calling `update`, passing this empty value on the first call. However, data
structures exist that can't reasonably represent an empty value.
