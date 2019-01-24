The `hello_world` example is intended to walk through the very basics
of a salsa setup. Here is a more detailed writeup.

### Step 1: Define a query group

A **query group** is a collection of queries (both inputs and
functions) that are defined in one particular spot. Each query group
represents some subset of the full set of queries you will use in your
application. Query groups can also depend on one another: so you might
have some basic query group A and then another query group B that uses
the queries from A and adds a few more. (These relationships must form
a DAG at present, but that is due to Rust's restrictions around
supertraits, which are likely to be lifted.)

Each query group is defined via a trait with the
`#[salsa::query_group]` decorator attached to it. `salsa::query_group`
is a procedural macro that will process the trait -- it will produce
not only the trait you specified, but also various additional types
you can later use and name.

```rust
#[salsa::query_group]
trait HelloWorldDatabase: salsa::Database {
    #[salsa::input]
    #[salsa::query_type(InputString)]
    fn input_string(&self, key: ()) -> Arc<String>;

    fn length(&self, key: ()) -> usize;
}
```

Each query group trait represents a self-contained block of queries
that can invoke each other and so forth. Your final database may
implement many such traits, thus combining many groups of queries into
the final program. Query groups are thus kind of analogous to Rust
crates: they represent a kind of "library" of queries that your final
program can use. Since we don't know the full set of queries that our
code may be combined with, when implementing a query group we don't
with a concrete database struct: instead we work against a generic
struct through traits, thus capturing the subset of functionality that
we actually need.

The `HelloWorldDatabase` trait has one supertrait:
`salsa::Database`. If we were defining more query groups in our
application, and we wanted to invoke some of those queries from within
this query group, we might list those query groups here. You can also
list any other traits you want, so long as your final database type
implements them (this lets you add custom state and so forth to your
database).

Within this trait, we list out the queries that this group provides.
Here, there are two: `input_string` and `length`. For each query, you
specify a function signature: the parameters to the function are
called the "key types" (in this case, we just give a single key of
type `()`) and the return type is the "value type". You can have any
number of key types. As you can see, though, this is not a real fn --
the "fn body" is obviously not real Rust syntax. Rather, it's just
used to specify a few bits of metadata about the query. We'll see how
to define the fn body in the next step.

**For each query.** For each query, the procedural macro will emit a
"query type", which is a kind of dummy struct that can be used to
refer to the query (we'll see an example of referencing this struct
later). For a query `foo_bar`, the struct is by default named
`FooBarQuery` -- but that name can be overridden with the
`#[salsa::query_type]` attribute. In our example, we override the
query type for `input_string` to be `InputString` but left `length`
alone (so it defaults to `LengthQuery`).

You can also use the `#[salsa::input]` attribute to designate
the "inputs" to the system. The values for input queries are not 
generated via a function but rather by explicit `set` operations,
as we'll see later. They are the starting points for your computation.

### Step 2: Define the query functions

Once you've defined your query group, you have to give the function
definition for every non-input query. In our case, that is the query
`length`. To do this, you simply define a function with the
appropriate name in the same module as the query group; if you would
prefer to use a different name or location, you add an attribute like
`#[salsa::invoke(path::to::other_fn)]` in the query definition to tell
us where to find it.

The query function for `length` looks like:

```rust
fn length(db: &impl HelloWorldDatabase, (): ()) -> usize {
    // Read the input string:
    let input_string = db.input_string(());

    // Return its length:
    input_string.len()
}
```

Note that every query function takes two arguments: the first is your
database, which you access via a generic that references your trait
(e.g., `impl HelloWorldDatabase`). The second is the key -- in this
simple example, that's just `()`.

**Invoking a query.** In the first line of the function we see how to
invoke a query for a given key:

```rust
let input_string = db.input_string(());
```

You simply call the function and give the key you want -- in this case
`()`.

### Step 3: Define the database struct

The final step is to create the **database struct** which will
implement the traits from each of your query groups. This struct
combines all the parts of your system into one whole; it can also add
custom state of your own (such as an interner or configuration). In
our simple example though we won't do any of that. The only field that
you **actually** need is a reference to the **salsa runtime**; then
you must also implement the `salsa::Database` trait to tell salsa
where to find this runtime:

```rust
#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseStruct> {
        &self.runtime
    }
}
```

Next, you must use the `database_storage!` to specify the set of query
groups that your database stores. This macro generates the internal
storage struct used to store your data. To use the macro, you
basically list out all the traits:

```rust
salsa::database_storage! {
    DatabaseStruct { // <-- name of your context type
        impl HelloWorldDatabase;
    }
}
```

The `database_storage` macro will also implement the
`HelloWorldDatabase` trait for your query context type.

**Use the database.** Now that we've defined our database, we can
start using it:

```rust
fn main() {
    let mut db = DatabaseStruct::default();

    println!("Initially, the length is {}.", db.length().get(()));

    db.query_mut(InputString)
        .set((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", db.length().get(()));
}
```

One thing to notice here is how we set the value for an input query:

```rust
    db.query_mut(InputString)
        .set((), Arc::new(format!("Hello, world")));
```

The `db.query_mut(Foo)` method takes as argument the query type that
characterizes your query. It gives back a "mutable query table" type,
which lets you invoke `set` to set the value of an input query. There
is also a `query` method that gives access to other advanced methods
in fact, the standard call `db.query_name(key)` to access a query is
just a shorthand for `db.query(QueryType).get(key)`.

Finally, if we run this code:

```bash
> cargo run --example hello_world
   Compiling salsa v0.2.0 (/Users/nmatsakis/versioned/salsa)
    Finished dev [unoptimized + debuginfo] target(s) in 0.94s
     Running `target/debug/examples/hello_world`
Initially, the length is 0.
Now, the length is 12.
```

Amazing.

