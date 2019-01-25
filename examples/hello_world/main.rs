use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////
// Step 1. Define the query group

// A **query group** is a collection of queries (both inputs and
// functions) that are defined in one particular spot. Each query
// group is defined by a representative struct (used internally by
// Salsa) as well as a representative trait. By convention, for a
// query group `Foo`, the struct is named `Foo` and the trait is named
// `FooDatabase`. The name `FooDatabase` reflects the fact that the
// trait is implemented by **the database**, which stores all the data
// in the system.  Each query group thus represents a subset of the
// full data.
//
// To define a query group, you annotate a trait definition with the
// `#[salsa::query_group(Foo)]` attribute macro. In addition to the
// trait definition, the macro will generate a struct with the name
// `Foo` that you provide, as well as various other bits of glue.
//
// Note that one query group can "include" another by listing the
// trait for that query group as a supertrait.
#[salsa::query_group(HelloWorld)]
trait HelloWorldDatabase: salsa::Database {
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

///////////////////////////////////////////////////////////////////////////
// Step 2. Define the queries.

// Define the **function** for the `length` query. This function will
// be called whenever the query's value must be recomputed. After it
// is called once, its result is typically memoized, unless we think
// that one of the inputs may have changed. Its first argument (`db`)
// is the "database", which is the type that contains the storage for
// all of the queries in the system -- we never know the concrete type
// here, we only know the subset of methods we care about (defined by
// the `HelloWorldDatabase` trait we specified above).
fn length(db: &impl HelloWorldDatabase, (): ()) -> usize {
    // Read the input string:
    let input_string = db.input_string(());

    // Return its length:
    input_string.len()
}

///////////////////////////////////////////////////////////////////////////
// Step 3. Define the database struct

// Define the actual database struct. This struct needs to be
// annotated with `#[salsa::database(..)]`. The list `..` will be the
// paths leading to the query group structs for each query group that
// this database supports. This attribute macro will generate the
// necessary impls so that the database implements the corresponding
// traits as well (so, here, `DatabaseStruct` will implement the
// `HelloWorldDatabase` trait).
//
// The database struct can contain basically anything you need, but it
// must have a `runtime` field as shown, and you must implement the
// `salsa::Database` trait (as shown below).
#[salsa::database(HelloWorld)]
#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

// Tell salsa where to find the runtime in your context.
impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseStruct> {
        &self.runtime
    }
}

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
