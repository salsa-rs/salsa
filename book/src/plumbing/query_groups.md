# Query groups and query group structs

When you define a query group trait:

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:trait}}
```

the `salsa::query_group` macro generates a number of things, shown in the sample
generated code below (details in the sections to come).

and associated storage struct) that represent things which don't have "public"
Note that there are a number of structs and types (e.g., the group descriptor
names. We currently generate mangled names with `__` afterwards, but those names
are not meant to be exposed to the user (ideally we'd use hygiene to enforce
this).

```rust,ignore
// First, a copy of the trait, though with extra supertraits and
// sometimes with some extra methods (e.g., `set_input_string`)
trait HelloWorld: 
    salsa::Database + 
    salsa::plumbing::HasQueryGroup<HelloWorldStorage>
{
    fn input_string(&self, key: ()) -> Arc<String>;
    fn set_input_string(&mut self, key: (), value: Arc<String>);
    fn length(&self, key: ()) -> usize;
}

// Next, the "query group struct", whose name was given by the
// user. This struct implements the `QueryGroup` trait which
// defines a few associated types common to the entire group.
struct HelloWorldStorage { }
impl salsa::plumbing::QueryGroup for HelloWorldStorage {
    type DynDb = dyn HelloWorld;
    type GroupStorage = HelloWorldGroupStorage__;
}

// Next, a blanket impl of the `HelloWorld` trait. This impl
// works for any database `DB` that implements the
// appropriate `HasQueryGroup`.
impl<DB> HelloWorld for DB
where
  DB: salsa::Database,
  DB: salsa::plumbing::HasQueryGroup<HelloWorldStorage>,
{
  ...
}

// Next, for each query, a "query struct" that represents it.
// The query struct has inherent methods like `in_db` and
// implements the `Query` trait, which defines various
// details about the query (e.g., its key, value, etc).
pub struct InputQuery { }
impl InputQuery { /* definition for `in_db`, etc */ }
impl salsa::Query for InputQuery {
    /* associated types */
}

// Same as above, but for the derived query `length`.
// For derived queries, we also implement `QueryFunction`
// which defines how to execute the query.
pub struct LengthQuery { }
impl salsa::Query for LengthQuery {
    ...
}
impl salsa::QueryFunction for LengthQuery {
    ...
}

// Finally, the group storage, which contains the actual
// hashmaps and other data used to implement the queries.
struct HelloWorldGroupStorage__ { .. }
```

## The group struct and `QueryGroup` trait

The group struct is the only thing we generate whose name is known to the user.
For a query group named `Foo`, it is conventionally called `FooStorage`, hence
the name `HelloWorldStorage` in our example.

Despite the name "Storage", the struct itself has no fields. It exists only to
implement the `QueryGroup` trait. This *trait* has a number of associated types
that reference various bits of the query group, including the actual "group
storage" struct:

```rust,ignore
struct HelloWorldStorage { }
impl salsa::plumbing::QueryGroup for HelloWorldStorage {
    type DynDb = dyn HelloWorld;
    type GroupStorage = HelloWorldGroupStorage__; // generated struct
}
```

We'll go into detail on these types below and the role they play, but one that
we didn't mention yet is `GroupData`. That is a kind of hack used to manage
send/sync around slots, and it gets covered in the section on slots.

## Impl of the hello world trait

Ultimately, every salsa query group is going to be implemented by your final
database type, which is not currently known to us (it is created by combining
multiple salsa query groups). In fact, this salsa query group could be composed
into multiple database types. However, we want to generate the impl of the query-group
trait here in this crate, because this is the point where the trait definition is visible
and known to us (otherwise, we'd have to duplicate the method definitions).

So what we do is that we define a different trait, called `plumbing::HasQueryGroup<G>`,
that can be implemented by the database type. `HasQueryGroup` is generic over
the query group struct. So then we can provide an impl of `HelloWorld` for any
database type `DB` where `DB: HasQueryGroup<HelloWorldStorage>`. This
`HasQueryGroup` defines a few methods that, given a `DB`, give access to the
data for the query group and a few other things.

Thus we can generate an impl that looks like:

```rust,ignore
impl<DB> HelloWorld for DB
where
    DB: salsa::Database,
    DB: salsa::plumbing::HasQueryGroup<HelloWorld>
{
    ...
    fn length(&self, key: ()) -> Arc<String> {
      <Self as salsa::plumbing::GetQueryTable<HelloWorldLength__>>::get_query_table(self).get(())
    }
}
```

You can see that the various methods just hook into generic functions in the
`salsa::plumbing` module. These functions are generic over the query types
(`HelloWorldLength__`) that will be described shortly. The details of the "query
table" are covered in a future section, but in short this code pulls out the
hasmap for storing the `length` results and invokes the generic salsa logic to
check for a valid result, etc.

## For each query, a query struct

As we referenced in the previous section, each query in the trait gets a struct
that represents it. This struct is named after the query, converted into snake
case and with the word `Query` appended. In typical Salsa workflows, these
structs are not meant to be named or used, but in some cases it may be required.
For e.g. the `length` query, this structs might look something like:

```rust,ignore
struct LengthQuery { }
```

The struct also implements the `plumbing::Query` trait, which defines
a bunch of metadata about the query (and repeats, for convenience,
some of the data about the group that the query is in):

```rust,ignore
{{#include ../../../components/salsa-macros/src/query_group.rs:Query_impl}}
```

Depending on the kind of query, we may also generate other impls, such as an
impl of `salsa::plumbing::QueryFunction`, which defines the methods for
executing the body of a query. This impl would then include a call to the user's
actual function.

```rust,ignore
{{#include ../../../components/salsa-macros/src/query_group.rs:QueryFunction_impl}}
```

## Group storage

The "group storage" is the actual struct that contains all the hashtables and
so forth for each query. The types of these are ultimately defined by the
`Storage` associated type for each query type. The struct is generic over the
final database type:

```rust,ignore
struct HelloWorldGroupStorage__ {
    input: <InputQuery as Query::Storage,
    length: <LengthQuery as Query>::Storage,
}
```

We also generate some inherent methods. First, a `new` method that takes
the group index as a parameter and passes it along to each of the query
storage `new` methods:

```rust,ignore
{{#include ../../../components/salsa-macros/src/query_group.rs:group_storage_new}}
```

And then various methods that will dispatch from a `DatabaseKeyIndex` that
corresponds to this query group into the appropriate query within the group.
Each has a similar structure of matching on the query index and then delegating
to some method defined by the query storage:

```rust,ignore
{{#include ../../../components/salsa-macros/src/query_group.rs:group_storage_methods}}
```