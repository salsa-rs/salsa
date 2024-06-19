# Tracked structs

Tracked structs are stored in a special way to reduce their costs.

Tracked structs are created via a `new` operation.

## The tracked struct and tracked field ingredients

For a single tracked struct we create multiple ingredients.
The **tracked struct ingredient** is the ingredient created first.
It offers methods to create new instances of the struct and therefore
has unique access to the interner and hashtables used to create the struct id.
It also shares access to a hashtable that stores the `ValueStruct` that
contains the field data.

For each field, we create a **tracked field ingredient** that moderates access
to a particular field. All of these ingredients use that same shared hashtable
to access the `ValueStruct` instance for a given id. The `ValueStruct`
contains both the field values but also the revisions when they last changed value.

## Each tracked struct has a globally unique id

This will begin by creating a _globally unique, 32-bit id_ for the tracked struct. It is created by interning a combination of

- the currently executing query;
- a u64 hash of the `#[id]` fields;
- a _disambiguator_ that makes this hash unique within the current query. i.e., when a query starts executing, it creates an empty map, and the first time a tracked struct with a given hash is created, it gets disambiguator 0. The next one will be given 1, etc.

## Each tracked struct has a `ValueStruct` storing its data

The struct and field ingredients share access to a hashmap that maps
each field id to a value struct:

```rust,ignore
{{#include ../../../src/tracked_struct.rs:ValueStruct}}
```

The value struct stores the values of the fields but also the revisions when
that field last changed. Each time the struct is recreated in a new revision,
the old and new values for its fields are compared and a new revision is created.

## The macro generates the tracked struct `Configuration`

The "configuration" for a tracked struct defines not only the types of the fields,
but also various important operations such as extracting the hashable id fields
and updating the "revisions" to track when a field last changed:

```rust,ignore
{{#include ../../../src/tracked_struct.rs:Configuration}}
```
