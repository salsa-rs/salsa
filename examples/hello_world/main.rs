use salsa::Database;
use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////
// Step 1. Define the query group

// A **query group** is a collection of queries (both inputs and
// functions) that are defined in one particular spot. Each query group
// represents some subset of the full set of queries you will use in your
// application. Query groups can also depend on one another: so you might
// have some basic query group A and then another query group B that uses
// the queries from A and adds a few more. (These relationships must form
// a DAG at present, but that is due to Rust's restrictions around
// supertraits, which are likely to be lifted.)
#[salsa::query_group]
trait HelloWorldDatabase: salsa::Database {
    // For each query, we give the name, input type (here, `()`)
    // and the output type `Arc<String>`. We can use attributes to
    // give other configuration:
    //
    // - `salsa::input` indicates that this is an "input" to the system,
    //   which must be explicitly set.
    // - `salsa::query_type` controls the name of the dummy struct
    //   that represents this query. We'll see it referenced
    //   later. The default would have been `InputStringQuery`.
    #[salsa::input]
    #[salsa::query_type(InputString)]
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
// annotated with `#[salsa::database(..)]`, which contains a list of
// query groups that this database supports. This attribute macro will generate
// the necessary impls so that the database implements all of those traits.
//
// The database struct can contain basically anything you need, but it
// must have a `runtime` field as shown, and you must implement the
// `salsa::Database` trait (as shown below).
#[salsa::database(HelloWorldDatabase)]
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

    // You cannot access input_string yet, because it does not have a value. If you do, it will
    // panic. You could create an Option interface by maintaining a HashSet of inserted keys.
    // println!("Initially, the length is {}.", db.length(()));

    db.query_mut(InputString)
        .set((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", db.length(()));
}
