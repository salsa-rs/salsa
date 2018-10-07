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
salsa::query_group! {
    trait HelloWorldDatabase: salsa::Database {
        // For each query, we give the name, input type (here, `()`)
        // and the output type `Arc<String>`. Inside the "fn body" we
        // give some other configuration.
        fn input_string(key: ()) -> Arc<String> {
            // The type we will generate to represent this query.
            type InputString;

            // Specify the queries' "storage" -- in this case, this is
            // an *input query*, which means that its value changes
            // only when it is explicitly *set* (see the `main`
            // function below).
            storage input;
        }

        // This is a *derived query*, meaning its value is specified by
        // a function (see Step 2, below).
        fn length(key: ()) -> usize {
            type Length;

            // No explicit storage defaults to `storage memoized;`
            //
            // The function that defines this query is (by default) a
            // function with the same name as the query in the
            // containing module (e.g., `length`).
        }
    }
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

// Define the actual database struct. This must contain a salsa
// runtime but can also contain anything else you need.
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

// Define the full set of queries that your context needs. This would
// in general combine (and implement) all the database traits in
// your application into one place, allocating storage for all of
// them.
salsa::database_storage! {
    struct DatabaseStorage for DatabaseStruct {
        impl HelloWorldDatabase {
            fn input_string() for InputString;
            fn length() for Length;
        }
    }
}

// This shows how to use a query.
fn main() {
    let db = DatabaseStruct::default();

    println!("Initially, the length is {}.", db.length(()));

    db.query(InputString)
        .set((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", db.length(()));
}
