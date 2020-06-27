use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////
// Step 1. Define the query group

// A **query group** is a collection of queries (both inputs and
// functions) that are defined in one particular spot. Each query
// group is defined by a trait decorated with the
// `#[salsa::query_group]` attribute. The trait defines one method per
// query, with the arguments to the method being the query **keys** and
// the return value being the query's **value**.
//
// Along with the trait, each query group has an associated
// "storage struct". The name of this struct is specified in the `query_group`
// attribute -- for a query group `Foo`, it is conventionally `FooStorage`.
//
// When we define the final database (see below), we will list out the
// storage structs for each query group that it contains. The database
// will then automatically implement the traits.
//
// Note that one query group can "include" another by listing the
// trait for that query group as a supertrait.
// ANCHOR:trait
#[salsa::query_group(HelloWorldStorage)]
trait HelloWorld: salsa::Database {
    // For each query, we give the name, some input keys (here, we
    // have one key, `()`) and the output type `Arc<String>`. We can
    // use attributes to give other configuration:
    //
    // - `salsa::input` indicates that this is an "input" to the system,
    //   which must be explicitly set. The `salsa::query_group` method
    //   will autogenerate a `set_input_string` method that can be
    //   used to set the input.
    #[salsa::input]
    fn input_string(&self, key: ()) -> Arc<String>;

    // This is a *derived query*, meaning its value is specified by
    // a function (see Step 2, below).
    fn length(&self, key: ()) -> usize;
}
// ANCHOR_END:trait

///////////////////////////////////////////////////////////////////////////
// Step 2. Define the queries.

// Define the **function** for the `length` query. This function will
// be called whenever the query's value must be recomputed. After it
// is called once, its result is typically memoized, unless we think
// that one of the inputs may have changed. Its first argument (`db`)
// is the "database", which is the type that contains the storage for
// all of the queries in the system -- we never know the concrete type
// here, we only know the subset of methods we care about (defined by
// the `HelloWorld` trait we specified above).
fn length(db: &impl HelloWorld, (): ()) -> usize {
    // Read the input string:
    let input_string = db.input_string(());

    // Return its length:
    input_string.len()
}

///////////////////////////////////////////////////////////////////////////
// Step 3. Define the database struct

// Define the actual database struct. This struct needs to be
// annotated with `#[salsa::database(..)]`. The list `..` will be the
// paths leading to the storage structs for each query group that this
// database supports. This attribute macro will generate the necessary
// impls so that the database implements the corresponding traits as
// well (so, here, `DatabaseStruct` will implement the `HelloWorld`
// trait).
//
// The database struct can contain basically anything you need, but it
// must have a `runtime` field as shown, and you must implement the
// `salsa::Database` trait (as shown below).
// ANCHOR:database
#[salsa::database(HelloWorldStorage)]
#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

// Tell salsa where to find the runtime in your context.
impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}
// ANCHOR_END:database

// This shows how to use a query.
fn main() {
    let mut db = DatabaseStruct::default();

    // You cannot access input_string yet, because it does not have a
    // value. If you do, it will panic. You could create an Option
    // interface by maintaining a HashSet of inserted keys.
    // println!("Initially, the length is {}.", db.length(()));

    db.set_input_string((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", db.length(()));
}
