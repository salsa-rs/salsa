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

Each query group is defined via an invocation of `salsa::query_group!`
macro. This is the invocation used in `hello_world`:

```rust
salsa::query_group! {
    trait HelloWorldDatabase: salsa::Database {
        fn input_string(key: ()) -> Arc<String> {
            type InputString;
            storage input;
        }

        fn length(key: ()) -> usize {
            type Length;
        }
    }
}
```

This invocation will in fact expand to a number of things you can
later use and name. First and foremost is the **query group trait**,
here called `HelloWorldDatabase`. As the name suggests, this trait
will ultimately be implemented by the **database**, which is the
struct in your application that contains the store for all queries and
any other global state that persists beyond a single query execution.
In writing your application, though, we never work with a concrete
database struct: instead we work against a generic struct through
traits, thus capturing the subset of functionality that we actually
need.

The `HelloWorldDatabase` trait has one supertrait:
`salsa::Database`. If we were defining more query groups in our
application, and we wanted to invoke some of those queries from within
this query group, we might list those query groups here. You can also
list any other traits you want, so long as your final database type
implements them (this lets you add custom state and so forth to your
database).

Within this trait, we list out the queries that this group provides.
Here, there are two: `input_string` and `length`. For each query, you
specify the key and value type of the query in the form of a function:
but the "fn body" is obviously not real Rust syntax. Rather, it's just
used to specify a few bits of metadata about the query. We'll see how
to define the fn body in the next step.

**For each query.** For each query, we must **always** define a `type`
(e.g., `type InputString;`).  The macro will define a type with this
name alongside the trait: you can use this name later to specify which
query you are talking about. This is needed for some of the more
advanced methods (we'll discuss them later).

You can also optionally define the **storage** for a query via a
declaration like `storage <s>;`. The most common kind of storage is
either *memoized* (the default) or *input*. An *input* is a special
sort of query that is not defined by a function: rather, it gets its
values via explicit `set` operations (we'll see them later). In our
case, we define one input query (`input_string`) and one memoized
query (`length`).

### Step 2: Define the query functions

Once you've defined your query group, you have to give the function
definition for every non-input query. In our case, that is the query
`length`. To do this, you simply define a function with the
appropriate name in the same module as the query group; if you would
prefer to use a different name or location, you write `use fn
path::to::other_fn;` in the query definition to tell us where to find
it.

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

Next, you must use the `database_storage!` to define the "storage
struct" for your type. This storage struct contains all the hashmaps
and other things that salsa uses to store the values for your
queries. You won't need to interact with it directly. To use the
macro, you basically list out all the traits and each of the queries
within those traits:

```rust
salsa::database_storage! {
    struct DatabaseStorage for DatabaseStruct {
    //     ^^^^^^^^^^^^^^^     --------------
    //     name of the type    the name of your context type
    //     we will make
        impl HelloWorldDatabase {
            fn input_string() for InputString;
            fn length() for Length;
        }
    }
}
```

The `database_storage` macro will also implement the
`HelloWorldDatabase` trait for your query context type.

**Use the database.** Now that we've defined our database, we can
start using it:

```rust
fn main() {
    let db = DatabaseStruct::default();

    println!("Initially, the length is {}.", db.length().get(()));

    db.query(InputString)
        .set((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", db.length().get(()));
}
```

One thing to notice here is how we set the value for an input query:

```rust
    db.query(InputString)
        .set((), Arc::new(format!("Hello, world")));
```

The `db.query(Foo)` method takes as argument the query type that
characterizes your query. It gives back a "query table" type, which
offers you more advanced methods beyond simply executing the query
(for example, for input queries, you can invoke `set`). In fact, the
standard call `db.query_name(key)` to access a query is just a
shorthand for `db.query(QueryType).get(key)`.

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

