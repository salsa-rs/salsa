# The `'db` lifetime

[Tracked](./tracked_structs.md) and interned structs are both declared with a `'db` lifetime.
This lifetime is linked to the `db: &DB` reference used to create them.
It prevents the user from creating a new Salsa revision while a tracked or interned struct is in use.
Creating a new revision requires modifying an input through an `&mut DB` reference, which cannot coexist with the `&DB` borrow for `'db`.
References returned by field getters are tied to the same lifetime.

## The user type contains an id

The `#[salsa::tracked]` macro creates a user-exposed struct that looks roughly like this:

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct MyTrackedStruct<'db>(
    salsa::Id,
    std::marker::PhantomData<fn() -> &'db ()>,
);
```

The `PhantomData` carries the `'db` lifetime but does not point to the struct's fields.
The `salsa::Id` identifies a slot in Salsa's paged table, where the fields and their revision metadata are stored.
A field getter uses the id and the database to find that slot.
Reading a field annotated with `#[tracked]` also records a dependency in the active query.

## Across revisions

Fields without `#[tracked]` determine a tracked struct's identity.
Fields annotated with `#[tracked]` do not affect its identity and may be updated when the defining query creates the struct again in a later revision.
If the defining query no longer creates the struct, Salsa can reclaim its table slot.
When a slot is reused, the generation in its `salsa::Id` is incremented so that the new value has a different id.

Interned structs use the same basic `salsa::Id` and `PhantomData` representation, though their identity and reclamation rules differ from tracked structs.
