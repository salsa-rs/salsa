# salsa

*A generic framework for on-demand, incrementalized computation.*

## Obligatory warning

Very much a WORK IN PROGRESS at this point. Ready for experimental use
but expect frequent breaking changes.

## Credits

This system is heavily inspired by adapton, glimmer, and rustc's query
system. So credit goes to Eduard-Mihai Burtescu, Matthew Hammer,
Yehuda Katz, and Michael Woerister.

## Key idea

The key idea of `salsa` is that you define two things:

- **Inputs**: the base inputs to your system. You can change these
  whenever you like.
- **Queries**: values derived from those inputs. These are defined via
  "pure functions" (no side effects). The results of queries can be
  memoized to avoid recomputing them a lot. When you make changes to
  the inputs, we'll figure out (fairly intelligently) when we can
  re-use these memoized values and when we have to recompute them.

## How to use Salsa in three easy steps

Using salsa is as easy as 1, 2, 3...

1. Define one or more **query context traits** that contain the inputs
   and queries you will need. We'll start with one such trait, but
   later on you can use more than one to break up your system into
   components (or spread your code across crates).
2. **Implement the queries** using the `query_definition!` macro.
3. Create the **query context implementation**, which contains a full
   listing of all the inputs/queries you will be using. The query
   content implementation will contain the storage for all of the
   inputs/queries and may also contain anything else that your code
   needs (e.g., configuration data).
  
Let's walk through an example! This is [the `hello_world`
example](examples/hello_world) from the repository.

### Step 1: Define a query context trait

The "query context" is the central struct that holds all the state for
your application. It has the current values of all your inputs, the
values of any memoized queries you have executed thus far, and
dependency information between them.

```rust
pub trait HelloWorldContext: salsa::QueryContext {
    salsa::query_prototype! {
        /// The fundamental **input** to the system: contains a
        /// complete list of files.
        fn all_files() for AllFiles;

        /// A **derived value**: filtered list of paths representing
        /// jpegs.
        fn jpegs() for Jpegs;

        /// A **derived value**: the size of the biggest image. To
        /// avoid doing actual image manipulating, we'll use the silly
        /// metric of the longest file name. =)
        fn largest() for Largest;
    }
}
```

### 

Let's make a very simple, hello-world sort of example. We'll make two inputs,
each of whihc is 


## Goals

It tries to hit a few goals:

- No need for a base crate that declares the "complete set of queries"
- Each query can define its own storage and doesn't have to be memoized
- Each module only has to know about the queries that it depends on
  and that it provides (but no others)
- Compiles to fast code, with no allocation, dynamic dispatch, etc on
  the "memoized hit" fast path
- Can recover from cycles gracefully (though I didn't really show
  that)
- Should support arenas and other lifetime-based things without requiring
  lifetimes everywhere when you're not using them (untested)

## Example

There is a working `hello_world` example which is probably the best documentation.
More to come when I expand out a few more patterns.
