# Database

Continuing our dissection, the other thing which a user must define is a
**database**, which looks something like this:

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:database}}
```

The `salsa::database` procedural macro takes a list of query group
structs (like `HelloWorldStorage`) and generates the following items:

* a copy of the database struct it is applied to
* a struct `__SalsaDatabaseStorage` that contains all the storage structs for
  each query group. Note: these are the structs full of hashmaps etc that are
  generaetd by the query group procdural macro, not the `HelloWorldStorage`
  struct itself.
* a struct `__SalsaDatabaseKey` that wraps an enum `__SalsaDatabaseKeyKind`. The
  enum contains one variant per query group, and in each variant contains the
  group key. This can be used to identify any query in the database.
* an impl of `HasQueryGroup<G>` for each query group `G`
* an impl of `salsa::plumbing::DatabaseStorageTypes` for the database struct
* an impl of `salsa::plumbing::DatabaseOps` for the database struct
* an impl of `salsa::plumbing::DatabaseKey<DB>` for the database struct `DB`

## Key constraint: we do not know the names of individual queries

There is one key constraint in the design here. None of this code knows the
names of individual queries. It only knows the name of the query group storage
struct. This means that we often delegate things to the group -- e.g., the
database key is composed of group keys. This is similar to how none of the code
in the query group knows the full set of query groups, and so it must use
associated types from the `Database` trait whenever it needs to put something in
a "global" context.

## The database storage struct

The `__SalsaDatabaseStorage` struct concatenates all of the query group storage
structs. In the hello world example, it looks something like:

```rust
struct __SalsaDatabaseStorage {
    hello_world: <HelloWorldStorage as salsa::plumbing::QueryGroup<DatabaseStruct>>::GroupStorage
}
```

## The database key struct / enum and the `DatabaseKey` impl

The `__SalsaDatabaseKey` and `__SalsaDatabaseKeyKind` types create a **database
key**, which uniquely identifies any query in the database. It builds on the
**group keys** created by the query groups, which uniquely identify a query
within a given query group.

```rust
struct __SalsaDatabaseKey {
    kind: __SalsaDatabaseKeyKind
}

enum __SalsaDatabaseKeyKind {
    HelloWorld(
        <HelloWorldStorage as salsa::plumbing::QueryGroup<DatabaseStruct>>::GroupKey
    )
}
```

We also generate an impl of `DatabaseKey`:

```rust,ignore
{{#include ../../../components/salsa-macros/src/database_storage.rs:DatabaseKey}}
```

## The `HasQueryGroup` impl

The `HasQueryGroup` trait allows a given query group to access its definition
within the greater database. The impl is generated here:

```rust,ignore
{{#include ../../../components/salsa-macros/src/database_storage.rs:HasQueryGroup}}
```

and so for our example it would look something like

```rust
impl salsa::plumbing::HasQueryGroup<HelloWorld> for DatabaseStruct {
    fn group_storage(&self) -> &HelloWorldStorage::GroupStorage {
        &self.hello_world
    }

    fn database_key(group_key: HelloWorldStorage::GroupKey) -> __SalsaDatabaseKey {
        __SalsaDatabaseKey {
            kind: __SalsaDatabaseKeyKind::HelloWorld(group_key)
        }
    }
}
```

## Other impls

Then there are a variety of other impls, like this one for `DatabaseStorageTypes`:

```rust,ignore
{{#include ../../../components/salsa-macros/src/database_storage.rs:DatabaseStorageTypes}}
```

Or this one for `DatabaseOps`, which defines the for-each method to
invoke an operation on every kind of query in the database. It ultimately
delegates to the `for_each` methods for the groups:

```rust,ignore
{{#include ../../../components/salsa-macros/src/database_storage.rs:DatabaseOps}}
```
