# Summary

- We introduce `#[salsa::interned]` queries which convert a `Key` type
  into a numeric index of type `Value`, where `Value` is either the
  type `InternId` (defined by a salsa) or some newtype thereof.
- Each interned query `foo` also produces an inverse `lookup_foo`
  method that converts back from the `Value` to the `Key` that was
  interned.
- The `InternId` type (defined by salsa) is basically a newtype'd integer,
  but it internally uses `NonZeroU32` to enable space-saving optimizations
  in memory layout.
- The `Value` types can be any type that implements the
  `salsa::InternIndex` trait, also introduced by this RFC. This trait
  has two methods, `from_intern_id` and `as_intern_id`.
- The interning is integrated into the GC and tracked like any other
  query, which means that interned values can be garbage-collected,
  and any computation that was dependent on them will be collected.

# Motivation

## The need for interning

Many salsa applications wind up needing the ability to construct
"interned keys". Frequently this pattern emerges because we wish to
construct identifiers for things in the input. These identifiers
generally have a "tree-like shape". For example, in a compiler, there
may be some set of input files -- these are enumerated in the inputs
and serve as the "base" for a path that leads to items in the user's
input. But within an input file, there are additional structures, such
as `struct` or `impl` declarations, and these structures may contain
further structures within them (such as fields or methods). This gives
rise to a path like so that can be used to identify a given item:

```notrust
PathData = <file-name>
         | PathData / <identifier>
```

These paths *could* be represented in the compiler with an `Arc`, but
because they are omnipresent, it is convenient to intern them instead
and use an integer. Integers are `Copy` types, which is convenient,
and they are also small (32 bits typically suffices in practice).

## Why interning is difficult today: garbage collection

Unfortunately, integrating interning into salsa at present presents
some hard choices, particularly with a long-lived application. You can
easily add an interning table into the database, but unless you do
something clever, **it will simply grow and grow forever**. But as the
user edits their programs, some paths that used to exist will no
longer be relevant -- for example, a given file or impl may be
removed, invalidating all those paths that were based on it. 

Due to the nature of salsa's recomputation model, it is not easy to
detect when paths that used to exist in a prior revision are no longer
relevant in the next revision. **This is because salsa never
explicitly computes "diffs" of this kind between revisions -- it just
finds subcomputations that might have gone differently and re-executes
them.** Therefore, if the code that created the paths (e.g., that
processed the result of the parser) is part of a salsa query, it will
simply not re-create the invalidated paths -- there is no explicit
"deletion" point.

In fact, the same is true of all of salsa's memoized query values. We
may find that in a new revision, some memoized query values are no
longer relevant. For example, in revision R1, perhaps we computed
`foo(22)` and `foo(44)`, but in the new input, we now only need to
compute `foo(22)`. The `foo(44)` value is still memoized, we just
never asked for its value. **This is why salsa includes a garbage
collector, which can be used to cleanup these memoized values that are
no longer relevant.**

But using a garbage collection strategy with a hand-rolled interning
scheme is not easy. You *could* trace through all the values in
salsa's memoization tables to implement a kind of mark-and-sweep
scheme, but that would require for salsa to add such a mechanism. It
might also be quite a lot of tracing! The current salsa GC mechanism has no
need to walk through the values themselves in a memoization table, it only
examines the keys and the metadata (unless we are freeing a value, of course).

## How this RFC changes the situation

This RFC presents an alternative. The idea is to move the interning
into salsa itself by creating special "interning
queries". Dependencies on these queries are tracked like any other
query and hence they integrate naturally with salsa's garbage
collection mechanisms.

# User's guide

This section covers how interned queries are expected to be used.

## Declaring an interned query

You can declare an interned query like so:

```rust,ignore
#[salsa::query_group]
trait Foo {
  #[salsa::interned]
  fn intern_path_data(&self, data: PathData) -> salsa::InternId;
]
```

**Query keys.** Like any query, these queries can take any number of keys. If multiple
keys are provided, then the interned key is a tuple of each key
value. In order to be interned, the keys must implement `Clone`,
`Hash` and `Eq`. 

**Return type.** The return type of an interned key may be of any type
that implements `salsa::InternIndex`: salsa provides an impl for the
type `salsa::InternId`, but you can implement it for your own.

**Inverse query.** For each interning query, we automatically generate
a reverse query that will invert the interning step. It is named
`lookup_XXX`, where `XXX` is the name of the query. Hence here it
would be `fn lookup_intern_path(&self, key: salsa::InternId) -> Path`.

## The expected us

Using an interned query is quite straightforward. You simply invoke it
with a key, and you will get back an integer, and you can use the
generated `lookup` method to convert back to the original value:

```rust,ignore
let key = db.intern_path(path_data1);
let path_data2 = db.lookup_intern_path_data(key);
```

Note that the interned value will be cloned -- so, like all Salsa
values, it is best if that is a cheap operation. Interestingly,
interning can help to keep recursive, tree-shapes values cheap,
because the "pointers" within can be replaced with interned keys.

## Custom return types

The return type for an intern query does not have to be a `InternId`. It can
be any type that implements the `salsa::InternKey` trait:

```rust,ignore
pub trait InternKey {
    /// Create an instance of the intern-key from a `InternId` value.
    fn from_intern_id(v: InternId) -> Self;

    /// Extract the `InternId` with which the intern-key was created.
    fn as_intern_id(&self) -> InternId;
}
```

## Recommended practice

This section shows the recommended practice for using interned keys,
building on the `Path` and `PathData` example that we've been working
with. 

### Naming Convention

First, note the recommended naming convention: the *intern key* is
`Foo` and the key's associated data `FooData` (in our case, `Path` and
`PathData`). The intern key is given the shorter name because it is
used far more often. Moreover, other types should never store the full
data, but rather should store the interned key.

### Defining the intern key

The intern key should always be a newtype struct that implements
the `InternKey` trait. So, something like this:

```rust,ignore
pub struct Path(InternId);

impl salsa::InternKey for Path {
    fn from_intern_id(v: InternId) -> Self {
        Path(v)
    }

    fn as_intern_id(&self) -> InternId {
        self.0
    }
}
```

### Convenient lookup method

It is often convenient to add a `lookup` method to the newtype key:

```rust,ignore
impl Path {
    // Adding this method is often convenient, since you can then
    // write `path.lookup(db)` to access the data, which reads a bit better.
    pub fn lookup(&self, db: &impl MyDatabase) -> PathData {
        db.lookup_intern_path_data(*self)
    }
}
```

### Defining the data type

Recall that our paths were defined by a recursive grammar like so:

```notrust
PathData = <file-name>
         | PathData / <identifier>
```

This recursion is quite typical of salsa applications. The recommended
way to encode it in the `PathData` structure itself is to build on other
intern keys, like so:

```rust,ignore
#[derive(Clone, Hash, Eq, ..)]
enum PathData {
  Root(String),
  Child(Path, String),
  //    ^^^^ Note that the recursive reference here
  //         is encoded as a Path.
}
```

Note though that the `PathData` type will be cloned whenever the value
for an interned key is looked up, and it may also be cloned to store
dependency information between queries. So, as an optimization, you
might prefer to avoid `String` in favor of `Arc<String>` -- or even
intern the strings as well.

## Interaction with the garbage collector

Interned keys can be garbage collected as normal, with one
caveat. Even if requested, Salsa will never collect the results
generated in the current revision. This is because it would permit the
same key to be interned twice in the same revision, possibly mapping
to distinct intern keys each time.

Note that if an interned key *is* collected, its index will be
re-used.  Salsa's dependency tracking system should ensure that
anything incorporating the older value is considered dirty, but you
may see the same index showing up more than once in the logs.

# Reference guide

Interned keys are implemented using a hash-map that maps from the
interned data to its index, as well as a vector containing (for each
index) various bits of data. In addition to the interned data, we must
track the revision in which the value was interned and the revision in
which it was last accessed, to help manage the interaction with the
GC. Finally, we have to track some sort of free list that tracks the
keys that are being re-used. The current implementation never actually
shrinks the vectors and maps from their maximum size, but this might
be a useful thing to be able to do (this is effectively a memory
allocator, so standard allocation strategies could be used here).

## InternId

Presently the `InternId` type is implemented to wrap a `NonZeroU32`:

```rust,ignore
pub struct InternId {
    value: NonZeroU32,
}
```

This means that `Option<InternId>` (or `Option<Path>`, continuing our
example from before) will only be a single word. To accommodate this,
the `InternId` constructors require that the value is less than
`InternId::MAX`; the value is deliberately set low (currently to
`0xFFFF_FF00`) to allow for more sentinel values in the future (Rust
doesn't presently expose the capability of having sentinel values
other than zero on stable, but it is possible on nightly).

# Alternatives and future work

None at present.
