# Runtime

This section documents the contents of the salsa crate. The salsa crate contains code that interacts with the [generated code] to create the complete "salsa experience". 

[generated code]: ./generated_code.md

## Major types

The crate has a few major types.

### The [`salsa::Storage`] struct

The [`salsa::Storage`] struct is what users embed into their database. It consists of two main parts:

* The "query store", which is the [generated storage struct](./database.md#the-database-storage-struct).
* The [`salsa::Runtime`].

### The [`salsa::Runtime`] struct

The [`salsa::Runtime`] struct stores the data that is used to track which queries are being executed and to coordinate between them. The `Runtime` is embedded within the [`salsa::Storage`] struct. 

**Important**. The `Runtime` does **not** store the actual data from the queries; they live alongside it in the [`salsa::Storage`] struct. This ensures that the type of `Runtime` is not generic which is needed to ensure dyn safety.

#### Threading

There is one [`salsa::Runtime`] for each active thread, and each of them has a unique [`RuntimeId`]. The `Runtime` state itself is divided into;

* `SharedState`, accessible from all runtimes;
* `LocalState`, accessible only from this runtime.

[`salsa::Runtime`]: https://docs.rs/salsa/latest/salsa/struct.Runtime.html 
[`salsa::Storage`]: https://docs.rs/salsa/latest/salsa/struct.Storage.html
[`RuntimeId`]: https://docs.rs/salsa/0.16.1/salsa/struct.RuntimeId.html

### Query storage implementations and support code

For each kind of query (input, derived, interned, etc) there is a corresponding "storage struct" that contains the code to implement it. For example, derived queries are implemented by the `DerivedStorage` struct found in the [`salsa::derived`] module.

[`salsa::derived`]: https://github.com/salsa-rs/salsa/blob/master/src/derived.rs

Storage structs like `DerivedStorage` are generic over a query type `Q`, which corresponds to the [query structs] in the generated code. The query structs implement the `Query` trait which gives basic info such as the key and value type of the query and its ability to recover from cycles. In some cases, the `Q` type is expected to implement additional traits: derived queries, for example, implement `QueryFunction`, which defines the code that will execute when the query is called.

[query structs]: ./query_groups.md#for-each-query-a-query-struct

The storage structs, in turn, implement key traits from the plumbing module. The most notable is the `QueryStorageOps`, which defines the [basic operations that can be done on a query](./query_ops.md).
