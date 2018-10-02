use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////
// Step 1. Define the query context trait

// Define a **query context trait** listing out all the prototypes
// that are defined in this section of the code (in real applications
// you would have many of these). For each query, we just give the
// name of the accessor method (`input_string`) and link that to a
// query type (`InputString`) that will be defined later.
salsa::query_prototype! {
    trait HelloWorldContext: salsa::QueryContext {
        fn input_string() for InputString;
        fn length() for Length;
    }
}

///////////////////////////////////////////////////////////////////////////
// Step 2. Define the queries.

// Define an **input query**. Like all queries, it is a map from a key
// (of type `()`) to a value (of type `Arc<String>`). All values begin
// as `Default::default` but you can assign them new values.
salsa::query_definition! {
    InputString: Map<(), Arc<String>>;
}

// Define a **function query**. It too has a key and value type, but
// it is defined with a function that -- given the key -- computes the
// value. This function is supplied with a context (an `&impl
// HelloWorldContext`) that gives access to other queries. The runtime
// will track which queries you use so that we can incrementally
// update memoized results.
salsa::query_definition! {
    Length(context: &impl HelloWorldContext, _key: ()) -> usize {
        // Read the input string:
        let input_string = context.input_string().get(());

        // Return its length:
        input_string.len()
    }
}

///////////////////////////////////////////////////////////////////////////
// Step 3. Implement the query context trait.

// Define the actual query context struct. This must contain a salsa
// runtime but can also contain anything else you need.
#[derive(Default)]
struct QueryContextStruct {
    runtime: salsa::runtime::Runtime<QueryContextStruct>,
}

// Tell salsa where to find the runtime in your context.
impl salsa::QueryContext for QueryContextStruct {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextStruct> {
        &self.runtime
    }
}

// Define the full set of queries that your context needs. This would
// in general combine (and implement) all the query context traits in
// your application into one place, allocating storage for all of
// them.
salsa::query_context_storage! {
    pub struct QueryContextStorage for QueryContextStruct {
        impl HelloWorldContext {
            fn input_string() for InputString;
            fn length() for Length;
        }
    }
}

// This shows how to use a query.
fn main() {
    let context = QueryContextStruct::default();

    println!("Initially, the length is {}.", context.length().get(()));

    context
        .input_string()
        .set((), Arc::new(format!("Hello, world")));

    println!("Now, the length is {}.", context.length().get(()));
}
