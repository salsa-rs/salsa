//! This crate provides salsa's macros and attributes.

#![recursion_limit = "128"]

extern crate proc_macro;
extern crate proc_macro2;
#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

mod query_group;

/// The decorator that defines a salsa "query group" trait. This is a
/// trait that defines everything that a block of queries need to
/// execute, as well as defining the queries themselves that are
/// exported for others to use.
///
/// This macro declares the "prototype" for a group of queries. It will
/// expand into a trait and a set of structs, one per query.
///
/// For each query, you give the name of the accessor method to invoke
/// the query (e.g., `my_query`, below), as well as its parameter
/// types and the output type. You also give the name for a query type
/// (e.g., `MyQuery`, below) that represents the query, and optionally
/// other details, such as its storage.
///
/// # Examples
///
/// The simplest example is something like this:
///
/// ```ignore
/// #[salsa::query_group]
/// trait TypeckDatabase {
///     #[salsa::input] // see below for other legal attributes
///     fn my_query(&self, input: u32) -> u64;
///
///     /// Queries can have any number of inputs (including zero); if there
///     /// is not exactly one input, then the key type will be
///     /// a tuple of the input types, so in this case `(u32, f32)`.
///     fn other_query(&self, input1: u32, input2: f32) -> u64;
/// }
/// ```
///
/// Here is a list of legal `salsa::XXX` attributes:
///
/// - Storage attributes: control how the query data is stored and set. These
///   are described in detail in the section below.
///   - `#[salsa::input]`
///   - `#[salsa::memoized]`
///   - `#[salsa::volatile]`
///   - `#[salsa::dependencies]`
/// - Query execution:
///   - `#[salsa::invoke(path::to::my_fn)]` -- for a non-input, this
///     indicates the function to call when a query must be
///     recomputed. The default is to call a function in the same
///     module with the same name as the query.
///   - `#[query_type(MyQueryTypeName)]` specifies the name of the
///     dummy struct created fo the query. Default is the name of the
///     query, in camel case, plus the word "Query" (e.g.,
///     `MyQueryQuery` and `OtherQueryQuery` in the examples above).
///
/// # Storage attributes
///
/// Here are the possible storage values for each query.  The default
/// is `storage memoized`.
///
/// ## Input queries
///
/// Specifying `storage input` will give you an **input
/// query**. Unlike derived queries, whose value is given by a
/// function, input queries are explicitly set by doing
/// `db.query(QueryType).set(key, value)` (where `QueryType` is the
/// `type` specified for the query). Accessing a value that has not
/// yet been set will panic. Each time you invoke `set`, we assume the
/// value has changed, and so we will potentially re-execute derived
/// queries that read (transitively) from this input.
///
/// ## Derived queries
///
/// Derived queries are specified by a function.
///
/// - `#[salsa::memoized]` (the default) -- The result is memoized
///   between calls.  If the inputs have changed, we will recompute
///   the value, but then compare against the old memoized value,
///   which can significantly reduce the amount of recomputation
///   required in new revisions. This does require that the value
///   implements `Eq`.
/// - `#[salsa::volatile]` -- indicates that the inputs are not fully
///   captured by salsa. The result will be recomputed once per revision.
/// - `#[salsa::dependencies]` -- does not cache the value, so it will
///   be recomputed every time it is needed. We do track the inputs, however,
///   so if they have not changed, then things that rely on this query
///   may be known not to have changed.
///
/// ## Attribute combinations
///
/// Some attributes are mutually exclusive. For example, it is an error to add
/// multiple storage specifiers:
///
/// ```compile_fail
/// # use salsa_macros as salsa;
/// #[salsa::query_group]
/// trait CodegenDatabase {
///     #[salsa::input]
///     #[salsa::memoized]
///     fn my_query(&self, input: u32) -> u64;
/// }
/// ```
///
/// It is also an error to annotate a function to `invoke` on an `input` query:
///
/// ```compile_fail
/// # use salsa_macros as salsa;
/// #[salsa::query_group]
/// trait CodegenDatabase {
///     #[salsa::input]
///     #[salsa::invoke(typeck::my_query)]
///     fn my_query(&self, input: u32) -> u64;
/// }
/// ```
#[proc_macro_attribute]
pub fn query_group(args: TokenStream, input: TokenStream) -> TokenStream {
    query_group::query_group(args, input)
}
