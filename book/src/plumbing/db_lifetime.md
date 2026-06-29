# The `'db` lifetime

[Tracked](./tracked_structs.md) and interned structs are both declared with a `'db` lifetime.
This lifetime is linked to the `db: &DB` reference used to create them.
The `'db` lifetime has several implications:

* It ensures that the user does not create a new salsa revision while a tracked/interned struct is in active use. Creating a new salsa revision requires modifying an input which requires an `&mut DB` reference, therefore it cannot occur during `'db`.
    * The struct may not even exist in the new salsa revision so allowing access would be confusing.
* It permits the structs to be implemented using a pointer rather than a `salsa::Id`, which in turn means more efficient field access (no read locks required).

This section discusses the unsafe code used for pointer-based access along with the reasoning behind it. To be concrete, we'll focus on tracked structs -- interned structs are very similar.

## A note on UB

When we say in this page "users cannot do X", we mean without Undefined Behavior (e.g., by transmuting integers around etc).

## Proof obligations

Here is a typical sequence of operations for a tracked struct along with the user operations that will require us to prove unsafe assertions:

* A tracked function `f` executes in revision R0 and creates a tracked struct with `#[id]` fields `K` for the first time.
    * `K` will be stored in the interning hashmap and mapped to a fresh identifier `id`.
    * The identifier `id` will be used as the key in the `StructMap` and point to a freshly created allocation `alloc : Alloc`.
    * A `ts: TS<'db>` is created from the raw pointer `alloc` and returned to the user.
* The value of the field `field` is accessed on the tracked struct instance `ts` by invoking the method `ts.field(db)`
    * *Unsafe:* This accesses the raw pointer to `alloc`.* A new revision R1 begins.
* The tracked function `f` does not re-execute in R1.
* The value of the field `field` is accessed on the tracked struct instance `ts` by invoking the method `ts.field(db)`
    * *Unsafe:* This accesses the raw pointer to `alloc`.* A new revision R2 begins.
* The tracked function `f` does reexecute in R2 and it again creates a tracked struct with key `K` and with (Some) distinct field values.
    * The fields for `ts` are updated.
* The value of the field `field` is accessed on the tracked struct instance `ts` by invoking the method `ts.field(db)`
    * *Unsafe:* This accesses the raw pointer to `alloc`.
* A new revision R3 begins.
* When `f` executes this time it does NOT create a tracked struct with key `K`. The tracked struct `ts` is placed in the "to be deleted" list.
* A new revision R4 begins:
    * The allocation `alloc` is freed.

As noted in the list, the core "unsafe" operation that users can perform is to access the fields of a tracked struct.
Tracked structs store a raw pointer to the `alloc`, owned by the ingredient, that contains their field data.
Accessing the fields of a tracked struct returns a `&`-reference to fields stored in that `alloc`, which means we must ensure Rust's two core constraints are satisfied for the lifetime of that reference:

* The allocation `alloc` will not be freed (i.e., not be dropped)
* The contents of the fields will not be mutated

As the sequence above illustrates, we have to show that those two constraints are true in a variety of circumstances:

* newly created tracked structs
* tracked structs that were created in prior revisions and re-validated in this revision
* tracked structs whose fields were updated in this revision
* tracked structs that were *not* created in this revision

## Definitions

For every tracked struct `ts` we say that it has a **defining query** `f(..)`. 
This refers to a particular invocation of the tracked function `f` with a particular set of arguments `..`.
This defining query is unique within a revision, meaning that `f` executes at most once with that same set of arguments.

We say that a query has *executed in a revision R* if its function body was executed. When this occurs, all tracked structs defined (created) by that query will be recorded along with the query's result.

We say that a query has been *validated in a revision R* if the salsa system determined that its inputs did not change and so skipped executing it. This also triggers the tracked structs defined by that query to be considered validated (in particular, we execute a function on them which updates some internal fields, as described below).

When we talk about `ts`, we mean 

## Theorem: At the start of a new revision, all references to `ts` are within salsa's database

After `ts` is deleted, there may be other memoized values still reference `ts`, but they must have a red input query.
**Is this true even if there are user bugs like non-deterministic functions?**
Argument: yes, because of non-forgery, those memoized values could not be accessed.
How did those memoized values obtain the `TS<'db>` value in the first place?
It must have come from a function argument (XX: what about thread-local state).
Therefore, to access the value, they would have to provide those function arguments again.
But how did they get them?

Potential holes:

* Thread-local APIs that let you thread `'db` values down in an "invisible" way, so that you can return them without them showing up in your arguments -- e.g. a tracked function `() -> S<'db>` that obtains its value from thread-local state.
    * We might be able to sanity check against this with enough effort by defining some traits that guarantee that every lifetime tagged thing in your result *could have* come from one of your arguments, but I don't think we can prove it altogether. We either have to tell users "don't do that" or we need to have some kind of dynamic check, e.g. with a kind of versioned pointer. Note that it does require unsafe code at present but only because of the limits of our existing APIs.
    * Alternatively we can do a better job cleaning up deleted stuff. This we could do.
* what about weird `Eq` implementations and the like? Do we have to make those unsafe?

## Theorem: To access a tracked struct `ts` in revision R, the defining query `f(..)` must have either *executed* or been *validated* in the revision R.

This is the core bit of reasoning underlying most of what follows.
The idea is that users cannot "forge" a tracked struct instance `ts`.
They must have gotten it through salsa's internal mechanisms.
This is important because salsa will provide `&`-references to fields within that remain valid during a revision.
But at the start of a new revision salsa may opt to modify those fields or even free the allocation.
This is safe because users cannot have references to `ts` at the start of a new revision.


### Lemma


We will prove it by proceeding through the revisions in the life cycle above (this can be considered a proof by induction).

### Before `ts` is first created in R0

Users must have originally obtained `ts: TS<'db>` by invoking `TS::new(&db, ...)`.
This is because creating an instance of `TS` requires providing a `NonNull<salsa::tracked_struct::ValueStruct>` pointer 
to an unsafe function whose contract requires the pointer's validity.

**FIXME:** This is not strictly true, I think the constructor is just a private tuple ctor, we should fix that.

### During R0 


### 


### Inductive case: Consider some revision R

We start by showing some circumstances that cannot occur:

* accessing the field of a tracked struct `ts` that was never created
* accessing the field of a tracked struct `ts` after it is freed

### Lemma (no forgery): Users cannot forge a tracked struct

The first observation is that users cannot "forge" an instance of a tracked struct `ts`.
They are required to produce a pointer to an `Alloc`.
This implies that every tracked struct `ts` originated in the ingredient.
The same is not true for input structs, for example, because they are created from integer identifiers and users could just make those up.

### Lemma (within one rev): Users cannot hold a tracked struct `ts` across revisions

The lifetime `'db` of the tracked struct `ts: TS<'db>` is created from a `db: &'db dyn Db` handle.
Beginning a new revision requires an `&mut` reference.
Therefore so long as users are actively using the value `ts` the database cannot start a new revision.

*Check:* What if users had two databases and invoked internal methods? Maybe they could then. We may have to add some assertions.

### Theorem: In order to get a tracked struct `ts` in revision R0, the tracked fn `f` that creates it must either *execute* or *be validated* first

The two points above combine to 


## Creating new values

Each new value is stored in a `salsa::alloc::Alloc` created by `StructMap::insert`.
`Alloc` is a variant of the standard Rust `Box` that carries no uniqueness implications.
This means that every tracked struct has its own allocation.
This allocation is owned by the tracked struct ingredient
and thus stays live until the tracked struct ingredient is dropped
or until it is removed (see later for safety conditions around removal).

## The user type uses a raw pointer

The `#[salsa::tracked]` macro creates a user-exposed struct that looks roughly like this:

```rust
// This struct is a wrapper around the actual fields that adds
// some revision metadata. You can think of it as a newtype'd
// version of the fields of the tracked struct.
use salsa::tracked_struct::ValueStruct;

struct MyTrackedStruct<'db> {
    value: *const ValueStruct<..>,
    phantom: PhantomData<&'db ValueStruct<...>>
}
```

Key observations:

* The actual pointer to the `ValueStruct` used at runtime is not a Rust reference but a raw pointer. This is needed for stacked borrows.
* A `PhantomData` is used to keep the `'db` lifetime alive.

The reason we use a raw pointer in the struct is because instances of this struct will outlive the `'db` lifetime. Consider this example:

```rust
let mut db = MyDatabase::default();
let input = MyInput::new(&db, ...);

// Revision 1:
let result1 = tracked_fn(&db, input);

// Revision 2:
input.set_field(&mut db).to(...);
let result2 = tracked_fn(&db, input);
```

Tracked structs created by `tracked_fn` during Revision 1
may be reused during Revision 2, but the original `&db` reference
used to create them has expired.
If we stored a true Rust reference, that would be a violation of
the stacked borrows rules.

Instead, we store a raw pointer and,
whenever users invoke the accessor methods for particular fields,
we create a new reference to the contents:

```rust
impl<'db> MyTrackedStruct<'db> {
    fn field(self, db: &'db dyn DB) -> &'db FieldType {
        ...
    }
}
```

This reference is linked to `db` and remains valid so long as the 

## The `'db` lifetime at rest

## Updating tracked struct fields across revisions

### The `XX`

## Safety lemmas

These lemmas are used to justify the safety of the system.

### Using `MyTracked<'db>` within some revision R always "happens after' a call to `MyTracked::new`

Whenever a tracked struct instance `TS<'db>` is created for the first time in revision R1,
the result is a fresh allocation and hence there cannot be any
pre-existing aliases of that struct.

`TS<'db>` will at that time be stored into the salsa database.
In later revisions, we assert that 

### `&'db T` references are never stored in the database


We maintain the invariant that, in any later revision R2, 

However in some later revision R2, how 

## Ways this could go wrong and how we prevent them

### 

### Storing an `&'db T` into a field


### Freeing the memory while a tracked struct remains live


### Aliases of a tracked struct
