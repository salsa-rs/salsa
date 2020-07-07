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
// is the "database". This is always a `&dyn` version of the query group
// trait, so that you can invoke all the queries you know about.
// We never know the concrete type here, as the full database may contain
// methods from other query groups that we don't know about.
fn length(db: &dyn HelloWorld, (): ()) -> usize {
    // Read the input string:
    let input_string = db.input_string(());

    // Return its length:
    input_string.len()
}

///////////////////////////////////////////////////////////////////////////
// Step 3. Define the database struct

// Define the actual database struct. This struct needs to be annotated with
// `#[salsa::database(..)]`. The list `..` will be the paths leading to the
// storage structs for each query group that this database supports. This
// attribute macro will generate the necessary impls so that the database
// implements the corresponding traits as well (so, here, `DatabaseStruct` will
// implement the `HelloWorld` trait).
//
// The database struct must have a field `storage: salsa::Storage<Self>`, but it
// can have any number of additional fields beyond that. The
// `#[salsa::database]` macro will generate glue code that accesses this
// `storage` field (but other fields are ignored). The `Storage<Self>` type
// contains all the actual hashtables and the like used to store query results
// and dependency information.
//
// In addition to including the `storage` field, you must also implement the
// `salsa::Database` trait (as shown below). This gives you a chance to define
// the callback methods within if you want to (in this example, we don't).
// ANCHOR:database
#[salsa::database(HelloWorldStorage)]
#[derive(Default)]
struct DatabaseStruct {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for DatabaseStruct {}
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
