# Selection

The "selection" (or "firewall") pattern is when you have a query Qsel that reads from some
other Qbase and extracts some small bit of information from Qbase that it returns.
In particular, Qsel does not combine values from other queries. In some sense,
then, Qsel is redundant -- you could have just extracted the information
the information from Qbase yourself, and done without the salsa machinery. But
Qsel serves a role in that it limits the amount of re-execution that is required
when Qbase changes.

## Example: the base query

For example, imagine that you have a query `parse` that parses the input text of a request
and returns a `ParsedResult`, which contains a header and a body:

```rust,ignore
{{#include ../../../examples/selection/main.rs:request}} 
```

## Example: a selecting query

And now you have a number of derived queries that only look at the header.
For example, one might extract the "content-type' header:

```rust,ignore
{{#include ../../../examples/selection/util1.rs:util1}} 
```

## Why prefer a selecting query?

This `content_type` query is an instance of the *selection* pattern. It only
"selects" a small bit of information from the `ParsedResult`. You might not have
made it a query at all, but instead made it a method on `ParsedResult`.

But using a query for `content_type` has an advantage: now if there are downstream
queries that only depend on the `content_type` (or perhaps on other headers extracted
via a similar pattern), those queries will not have to be re-executed when the request
changes *unless* the content-type header changes. Consider the dependency graph:

```text
request_text  -->  parse  -->  content_type  -->  (other queries)
``` 

When the `request_text` changes, we are always going to have to re-execute `parse`.
If that produces a new parsed result, we are *also* going to re-execute `content_type`.
But if the result of `content_type` has not changed, then we will *not* re-execute
the other queries.

## More levels of selection

In fact, in our example we might consider introducing another level of selection.
Instead of having `content_type` directly access the results of `parse`, it might be better
to insert a selecting query that just extracts the header:

```rust,ignore
{{#include ../../../examples/selection/util2.rs:util2}} 
```

This will result in a dependency graph like so:

```text
request_text  -->  parse  -->  header -->  content_type  -->  (other queries)
``` 

The advantage of this is that changes that only effect the "body" or
only consume small parts of the request will
not require us to re-execute `content_type` at all. This would be particularly
valuable if there are a lot of dependent headers.

## A note on cloning and efficiency

In this example, we used common Rust types like `Vec` and `String`,
and we cloned them quite frequently. This will work just fine in Salsa,
but it may not be the most efficient choice. This is because each clone
is going to produce a deep copy of the result. As a simple fix, you
might convert your data structures to use `Arc` (e.g., `Arc<Vec<ParsedHeader>>`),
which makes cloning cheap.

