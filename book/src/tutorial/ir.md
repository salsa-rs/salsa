# Defining the IR

Before we can define the [parser](./parser.md), we need to define the intermediate representation (IR) that we will use for `calc` programs.
In the [basic structure](./structure.md), we defined some "pseudo-Rust" structures like `Statement` and `Expression`;
now we are going to define them for real.

## "Salsa structs"

In addition to regular Rust types, we will make use of various **Salsa structs**.
A Salsa struct is a struct that has been annotated with one of the Salsa annotations:

- [`#[salsa::input]`](#input-structs), which designates the "base inputs" to your computation;
- [`#[salsa::tracked]`](#tracked-structs), which designate intermediate values created during your computation;
- [`#[salsa::interned]`](#interned-structs), which designate small values that are easy to compare for equality.

All Salsa structs store the actual values of their fields in the Salsa database.
This permits us to track when the values of those fields change to figure out what work will need to be re-executed.

When you annotate a struct with one of the above Salsa attributes, Salsa actually generates a bunch of code to link that struct into the database.
This code must be connected to some [jar](./jar.md).
By default, this is `crate::Jar`, but you can specify a different jar with the `jar=` attribute (e.g., `#[salsa::input(jar = MyJar)]`).
You must also list the struct in the jar definition itself, or you will get errors.

## Input structs

The first thing we will define is our **input**.
Every Salsa program has some basic inputs that drive the rest of the computation.
The rest of the program must be some deterministic function of those base inputs,
such that when those inputs change, we can try to efficiently recompute the new result of that function.

Inputs are defined as Rust structs with a `#[salsa::input]` annotation:

```rust
{{#include ../../../examples/calc/ir.rs:input}}
```

In our compiler, we have just one simple input, the `SourceProgram`, which has a `text` field (the string).

### The data lives in the database

Although they are declared like other Rust structs, Salsa structs are implemented quite differently.
The values of their fields are stored in the Salsa database and the struct themselves just reference it.
This means that the struct instances are copy (no matter what fields they contain).
Creating instances of the struct and accessing fields is done by invoking methods like `new` as well as getters and setters.

In the case of `#[salsa::input]`, the struct contains a `salsa::Id`, which is a non-zero integer.
Therefore, the generated `SourceProgram` struct looks something like this:

```rust
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceProgram(salsa::Id);
```

It will also generate a method `new` that lets you create a `SourceProgram` in the database.
For an input, a `&db` reference is required, along with the values for each field:

```rust
let source = SourceProgram::new(&db, "print 11 + 11".to_string());
```

You can read the value of the field with `source.text(&db)`,
and you can set the value of the field with `source.set_text(&mut db, "print 11 * 2".to_string())`.

### Database revisions

Whenever a function takes an `&mut` reference to the database,
that means that it can only be invoked from outside the incrementalized part of your program,
as explained in [the overview](../overview.md#goal-of-salsa).
When you change the value of an input field, that increments a 'revision counter' in the database,
indicating that some inputs are different now.
When we talk about a "revision" of the database, we are referring to the state of the database in between changes to the input values.

### Representing the parsed program

Next we will define a **tracked struct**.
Whereas inputs represent the _start_ of a computation, tracked structs represent intermediate values created during your computation.

In this case, the parser is going to take in the `SourceProgram` struct that we saw and return a `Program` that represents the fully parsed program:

```rust
{{#include ../../../examples/calc/ir.rs:program}}
```

Like with an input, the fields of a tracked struct are also stored in the database.
Unlike an input, those fields are immutable (they cannot be "set"), and Salsa compares them across revisions to know when they have changed.
In this case, if parsing the input produced the same `Program` result
(e.g., because the only change to the input was some trailing whitespace, perhaps),
then subsequent parts of the computation won't need to re-execute.
(We'll revisit the role of tracked structs in reuse more in future parts of the IR.)

Apart from the fields being immutable, the API for working with a tracked struct is quite similar to an input:

- You can create a new value by using `new`: e.g., `Program::new(&db, some_statements)`
- You use a getter to read the value of a field, just like with an input (e.g., `my_func.statements(db)` to read the `statements` field).
  - In this case, the field is tagged as `#[return_ref]`, which means that the getter will return a `&Vec<Statement>`, instead of cloning the vector.

### The `'db` lifetime

Unlike inputs, tracked structs carry a `'db` lifetime.
This lifetime is tied to the `&db` used to create them and
ensures that, so long as you are using the struct,
the database remains immutable:
in other words, you cannot change the values of a `salsa::Input`.

The `'db` lifetime also allows tracked structs to be implemented
using a pointer (versus the numeric id found in `salsa::input` structs).
This doesn't really effect you as a user except that it allows accessing fields from tracked structs—
a very common operation—to be optimized.

## Representing functions

We will also use a tracked struct to represent each function:
The `Function` struct is going to be created by the parser to represent each of the functions defined by the user:

```rust
{{#include ../../../examples/calc/ir.rs:functions}}
```

If we had created some `Function` instance `f`, for example, we might find that `the f.body` field changes
because the user changed the definition of `f`.
This would mean that we have to re-execute those parts of the code that depended on `f.body`
(but not those parts of the code that depended on the body of _other_ functions).

Apart from the fields being immutable, the API for working with a tracked struct is quite similar to an input:

- You can create a new value by using `new`: e.g., `Function::new(&db, some_name, some_args, some_body)`
- You use a getter to read the value of a field, just like with an input (e.g., `my_func.args(db)` to read the `args` field).

### id fields

To get better reuse across revisions, particularly when things are reordered, you can mark some entity fields with `#[id]`.
Normally, you would do this on fields that represent the "name" of an entity.
This indicates that, across two revisions R1 and R2, if two functions are created with the same name, they refer to the same entity, so we can compare their other fields for equality to determine what needs to be re-executed.
Adding `#[id]` attributes is an optimization and never affects correctness.
For more details, see the [algorithm](../reference/algorithm.md) page of the reference.

## Interned structs

The final kind of Salsa struct are _interned structs_.
As with input and tracked structs, the data for an interned struct is stored in the database.
Unlike those structs, if you intern the same data twice, you get back the **same integer**.

A classic use of interning is for small strings like function names and variables.
It's annoying and inefficient to pass around those names with `String` values which must be cloned;
it's also inefficient to have to compare them for equality via string comparison.
Therefore, we define two interned structs, `FunctionId` and `VariableId`, each with a single field that stores the string:

```rust
{{#include ../../../examples/calc/ir.rs:interned_ids}}
```

When you invoke e.g. `FunctionId::new(&db, "my_string".to_string())`, you will get back a `FunctionId` that is just a newtype'd integer.
But if you invoke the same call to `new` again, you get back the same integer:

```rust
let f1 = FunctionId::new(&db, "my_string".to_string());
let f2 = FunctionId::new(&db, "my_string".to_string());
assert_eq!(f1, f2);
```

### Interned values carry a `'db` lifetime

Like tracked structs, interned values carry a `'db` lifetime that prevents them from being used across salsa revisions.
It also permits them to be implemented using a pointer "under the hood", permitting efficient field access.
Interned values are guaranteed to be consistent within a single revision.
Across revisions, they may be cleared, reallocated, or reassigned -- but you cannot generally observe this,
since the `'db` lifetime prevents you from changing inputs (and hence creating a new revision)
while an interned value is in active use.

### Expressions and statements

We won't use any special "Salsa structs" for expressions and statements:

```rust
{{#include ../../../examples/calc/ir.rs:statements_and_expressions}}
```

Since statements and expressions are not tracked, this implies that we are only attempting to get incremental re-use at the granularity of functions --
whenever anything in a function body changes, we consider the entire function body dirty and re-execute anything that depended on it.
It usually makes sense to draw some kind of "reasonably coarse" boundary like this.

One downside of the way we have set things up: we inlined the position into each of the structs.
