# Tracked structs

Tracked structs are stored in a special way to reduce their costs.

Tracked structs are created via a `new` operation.

## The tracked struct and tracked field ingredients

For a single tracked struct we create multiple ingredients.
The **tracked struct ingredient** is the ingredient created first.
It creates new instances of the struct and assigns their ids.
The corresponding `ValueStruct` data is stored in Salsa's paged table.

For each `#[tracked]` field, we create a **tracked field ingredient** that moderates access
to a particular field. All of these ingredients use the same paged table
to access the `ValueStruct` instance for a given id. The `ValueStruct`
contains both the field values but also the revisions when they last changed value.

## Each tracked struct has an id

This begins by creating a database-local `salsa::Id` for the tracked struct.
The ID contains a table index and a generation used when slots are reused.
Its identity is derived from a combination of

- the currently executing query;
- a u64 hash of the fields not marked `#[tracked]`;
- a _disambiguator_ that makes this hash unique within the current query. i.e., when a query starts executing, it creates an empty map, and the first time a tracked struct with a given hash is created, it gets disambiguator 0. The next one will be given 1, etc.

## Each tracked struct has a `ValueStruct` storing its data

The struct and field ingredients use the paged table to find the value struct
for a given id:

```rust,ignore
{{#include ../../../src/tracked_struct.rs:ValueStruct}}
```

The value struct stores the values of the fields but also the revisions when
that field last changed. Each time the struct is recreated in a new revision,
the old and new values for its fields are compared and changed field revisions are updated.

## The macro generates the tracked struct `Configuration`

The "configuration" for a tracked struct defines not only the types of the fields,
but also various important operations such as extracting the hashable id fields
and updating the "revisions" to track when a field last changed:

```rust,ignore
{{#include ../../../src/tracked_struct.rs:Configuration}}
```
