# Query groups and query group structs

When you define a query group trait:

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:trait}}
```

the `salsa::query_group` macro generates a number of things:

* a copy of the `HelloWorld` trait, minus the salsa annotations, and lightly edited
* a "group struct" named `HelloWorldStorage` that represents the group; this struct implements `plumbing::QueryGroup`
  * somewhat confusingly, this struct doesn't actually contain the storage itself, but rather has an associated type that leads to the "true" storage struct
* an impl of the `HelloWorld` trait, for any database type
* for each query, a "query struct" named after the query; these structs implement `plumbing::Query` and sometimes other plumbing traits
* a group key, an enum that can identify any query within the group and store its key
* the associated storage struct, which contains the actual hashmaps that store the data for all queries in the group

Note that there are a number of structs and types (e.g., the group descriptor
and associated storage struct) that represent things which don't have "public"
names. We currently generate mangled names with `__` afterwards, but those names
are not meant to be exposed to the user (ideally we'd use hygiene to enforce
this).

So the generated code looks something like this. We'll go into more detail on
each part in the following sections.

```rust,ignore
// First, a copy of the trait, though sometimes with some extra
// methods (e.g., `set_input_string`)
trait HelloWorld: salsa::Database {
    fn input_string(&self, key: ()) -> Arc<String>;
    fn set_input_string(&mut self, key: (), value: Arc<String>);
    fn length(&self, key: ()) -> usize;
}

// Next, the group struct
struct HelloWorldStorage { }
impl<DB> salsa::plumbing::QueryGroup<DB> for HelloWorldStorage { ... }

// Next, the impl of the trait
impl<DB> HelloWorld for DB
where
  DB: salsa::Database,
  DB: salsa::plumbing::HasQueryGroup<HelloWorldStorage>,
{
  ...
}

// Next, a series of query structs and query impls
struct InputQuery { }
unsafe impl<DB> salsa::Query<DB> for InputQuery
where
    DB: HelloWorld,
    DB: salsa::plumbing::HasQueryGroup<#group_struct>,
    DB: salsa::Database,
{
    ...
}
struct LengthQuery { }
unsafe impl<DB> salsa::Query<DB> for LengthQuery
where
    DB: HelloWorld,
    DB: salsa::plumbing::HasQueryGroup<#group_struct>,
    DB: salsa::Database,
{
    ...
}

// For derived queries, those include implementations
// of additional traits like `QueryFunction`
unsafe impl<DB> salsa::QueryFunction<DB> for LengthQuery
where
    DB: HelloWorld,
    DB: salsa::plumbing::HasQueryGroup<#group_struct>,
    DB: salsa::Database,
{
    ...
}

// The group key
enum HelloWorldGroupKey__ { .. }

// The group storage
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
impl<DB> salsa::plumbing::QueryGroup<DB> for HelloWorldStorage {
    type GroupStorage = HelloWorldGroupStorage__; // generated struct
    type GroupKey = HelloWorldGroupKey__;
    type GroupData = ((), Arc<String>, (), usize);
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
unsafe impl<DB> salsa::Query<DB> for LengthQuery
where
    DB: HelloWorld,
    DB: salsa::plumbing::HasQueryGroup<#group_struct>,
    DB: salsa::Database,
{
    // A tuple of the types of the function parameters trom trait.
    type Key = ((), );

    // The return value of the function in the trait.
    type Value = Arc<String>;

    // The "query storage" references a type from within salsa
    // that stores the actual query data and defines the
    // logic for accessing and revalidating it.
    //
    // It is generic over the query type which lets it
    // customize itself to the keys/value of this particular
    // query.
    type Storage = salsa::derived::DerivedStorage<
        DB,
        LengthQuery,
        salsa::plumbing::MemoizedStorage,
    >;

    // Types from the query group, repeated for convenience.
    type Group = HelloWorldStorage;
    type GroupStorage = HelloWorldGroupStorage__;
    type GroupKey = HelloWorldGroupKey__;

    // Given the storage for the entire group, extract
    // the storage for just this query. Described when
    // we talk about group storage.
    fn query_storage(
        group_storage: &HelloWorldGroupStorage__,
    ) -> &std::sync::Arc<Self::Storage> {
        &group_storage.length
    }

    // Given the key for this query, construct the "group key"
    // that situates it within the group. Described when
    // we talk about group key.
    fn group_key(key: Self::Key) -> Self::GroupKey {
        HelloWorldGroupKey__::length(key)
    }
}
```

Depending on the kind of query, we may also generate other impls, such as an
impl of `salsa::plumbing::QueryFunction`, which defines the methods for
executing the body of a query. This impl would then include a call to the user's
actual function.

## Group key

The "query key" is the inputs to the query, and identifies a particular query
instace: in our example, it is a value of type `()` (so there is only one
instance of the query), but typically it's some other type. The "group key" then
broadens that to include the identifier of the query within the group. So instead
of just `()` the group key would encode (e.g.) `Length(())` (the "length" query
applied to the `()` key). It is represented as an enum, which we generate,
with one variant per query:

```rust,ignore
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum HelloWorldGroupKey__ {
  input(()),
  length(()),
}
```

The `Query` trait that we saw earlier includes a method `group_key` for wrapping
the key for some individual query into the group key.

## Group storage

The "group storage" is the actual struct that contains all the hashtables and
so forth for each query. The types of these are ultimately defined by the
`Storage` associated type for each query type. The struct is generic over the
final database type:

```rust,ignore
struct HelloWorldGroupStorage__<DB> {
    input: <InputQuery as Query<DB>>::Storage,
    length: <LengthQuery as Query<DB>>::Storage,
}
```

We also generate some impls: first is an impl of `Default` and the second is a
method `for_each_query` that simply iterates over each field and invokes a
method on it. This method is called by some of the code we generate for the
database in order to implement debugging methods that "sweep" over all the
queries.

```rust,ignore
impl<DB> HelloWorldGroupStorage__<DB> {
    fn for_each_query(&self, db: &DB, method: &mut dyn FnMut(...)) {
        ...
    }
}
```
