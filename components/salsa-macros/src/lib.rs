//! This crate provides salsa's macros and attributes.

#![recursion_limit = "256"]

#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

macro_rules! parse_quote {
    ($($inp:tt)*) => {
        {
            let tt = quote!{$($inp)*};
            syn::parse2(tt.clone()).unwrap_or_else(|err| {
                panic!("failed to parse `{}` at {}:{}:{}: {}", tt, file!(), line!(), column!(), err)
            })
        }
    }
}

/// Similar to `syn::parse_macro_input`, however, when a parse error is encountered, it will return
/// the input token stream in addition to the error. This will make it so that rust-analyzer can work
/// with incomplete code.
macro_rules! parse_macro_input {
    ($tokenstream:ident as $ty:ty) => {
        match syn::parse::<$ty>($tokenstream.clone()) {
            Ok(data) => data,
            Err(err) => {
                return $crate::token_stream_with_error($tokenstream, err);
            }
        }
    };
}

mod accumulator;
mod db;
mod db_lifetime;
mod debug;
mod fn_util;
mod hygiene;
mod input;
mod interned;
mod options;
mod salsa_struct;
mod supertype;
mod tracked;
mod tracked_fn;
mod tracked_impl;
mod tracked_struct;
mod update;
mod xform;

/// Collection of all attributes' documentation. Copy these and filter with
/// [`options::AllowedOptions`] and [`salsa_struct::SalsaStructAllowedOptions`] implementations.
///
/// # Container attributes
///
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `specify`: Indicate that the value can be externally specified (only works with a single Salsa struct as the input. Incompatible with `lru`)
// For functions:
/// - `no_eq`: Always mark the output as updated when function is re-created. The type does not have to implement [`Eq`]. This is incompatible with `cycle_fn`.
/// - `debug`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
// Explicitly not documented due to deprecation: - `no_lifetime`: TODO
// Explicitly not documented: - `unsafe(non_update_return_type)`
/// - `singleton`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `data = <ident>`: Name of the data type for an interned struct.
// Explicitly not documented as it's unused: - `db = <path>`: Path to the database.
// For functions:
/// - `cycle_fn = <path>`: Cycle recovery function. TODO (default: `salsa::plumbing::unexpected_cycle_recovery!`)
// For functions:
/// - `cycle_initial = <path>`: Initial value for cycle iteration. TODO (default: `salsa::plumbing::unexpected_cycle_initial!`)
// For functions:
/// - `cycle_result = <expr>`: Result for non-fixpoint cycle. TODO
/// - `lru = <usize>`: Set to a nonzero value to enable LRU (Least Recently Used) eviction of memoized values and set the LRU capacity. (default: 0)
/// - `constructor = <ident>`: Name of the constructor function. (default: `new`)
// Explicitly not documented: - `id = <path>`: custom ID for interned structs. Must implement `salsa::plumbing::AsId`. (default: `salsa::Id`)
/// - `revisions = <expr as usize>`: minimum number of revisions to keep a value interned.
///   (default: `salsa::plumbing::internal::Configuration::REVISIONS`)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `self_ty = <type>`: Set the self type of the tracked impl, merely to refine the query name.
/// - `persist` (Only with <span class="stab portability"><code>persistence</code></span> feature)
/// - `persist([serialize = <path>], [deserialize = <path>])` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_TRACKED`]:
/// - `tracked`: Marks the field as tracked. Fields without this attribute must implement [`Hash`](std::hash::Hash).
///   * Modifications to tracked fields only invalidates the data depending on the tracked fields. Use tracked fields when you need fine-grained incremental recomputation.
///   * Modifications to untracked fields invalidates everything depending on the whole tracked struct. Use untracked fields for identity-defining data that rarely changes.
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_DEFAULT`]:
/// - `default`: Marks the field as optional and as having a [`Default`] implementation.
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
// For input structs:
/// - `no_eq`: Always mark the field as updated when its setter is called. The type does not have to implement [`Eq`].
// For tracked structs:
/// - `no_eq`: Always mark the field as updated when the struct is recreated inside a tracked function. The type does not have to implement [`Eq`].
/// - `get(<ident>)`: Name of the getter function (default: field name)
// Only for inputs:
/// - `set(<ident>)`: Name of the setter function (default: `set_` + field name)
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_MAYBE_UPDATE`]:
// Explicitly not documented: - `maybe_update(<expr>)`: Function of type `unsafe fn(*mut #field_ty, #field_ty) -> bool`. TODO
mod attrs_doc {}

#[proc_macro_attribute]
pub fn accumulator(args: TokenStream, input: TokenStream) -> TokenStream {
    accumulator::accumulator(args, input)
}

/// Implements a custom database trait.
///
/// Apply this on a custom database trait's definition and the [`struct`] and [`impl`] items of
/// implementors.
///
/// When applied to [`struct`] items, this macro implements the necessary supertraits required for `salsa::Database`.
///
/// When applied to [`trait`] and [`impl`] items, this macro adds some hidden trait methods required for [`#[tracked]`](fn@tracked) functions.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
///
/// #[salsa::input]
/// struct File {
// Doesn't work without the std::path:: prefix...
///     path: std::path::PathBuf,
///     #[returns(ref)]
///     contents: String,
/// }
///
/// #[salsa::db]
/// trait Db: salsa::Database {
///     fn input(&self, path: PathBuf) -> std::io::Result<File>;
/// }
///
/// #[salsa::db]
/// #[derive(Clone)]
/// pub struct MyDatabase {
///    storage: salsa::Storage<Self>,
/// }
///
/// #[salsa::db]
/// impl salsa::Database for MyDatabase {}
///
/// #[salsa::db]
/// impl Db for MyDatabase {
///     fn input(&self, path: PathBuf) -> std::io::Result<File> {
///         todo!()
///     }
/// }
/// ```
///
/// [`struct`]: https://doc.rust-lang.org/std/keyword.struct.html
/// [`impl`]: https://doc.rust-lang.org/std/keyword.impl.html
/// [`trait`]: https://doc.rust-lang.org/std/keyword.trait.html
#[proc_macro_attribute]
pub fn db(args: TokenStream, input: TokenStream) -> TokenStream {
    db::db(args, input)
}

/// Creates interned structs.
///
/// **Interned structs** are dedpulicated, immutable structs used as parameters to tracked
/// functions.
///
/// # Container attributes
///
/// - `debug`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
// Explicitly not documented due to deprecation: - `no_lifetime`: TODO
/// - `singleton`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `data = <ident>`: TODO
/// - `constructor = <ident>`: Name of the constructor function. (default: `new`)
// Explicitly not documented: - `id = <path>`: TODO (default: `salsa::Id`)
/// - `revisions = <expr as usize>`: minimum number of revisions to keep a value interned.
///   (default: `salsa::plumbing::internal::Configuration::REVISIONS`)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `persist([serialize = <path>], [deserialize = <path>])` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `get(<ident>)`: Name of the getter function (default: field name)
///
/// # Example
///
/// ```
/// #[salsa::interned]
/// struct MyInterned<'db> {
///     field: String,
/// }
///
/// let db = salsa::DatabaseImpl::new();
/// let a = MyInterned::new(&db, "example");
/// let b = MyInterned::new(&db, "example");
///
/// // There is only one String allocation.
///
/// # drop((a, b));
/// ```
#[proc_macro_attribute]
pub fn interned(args: TokenStream, input: TokenStream) -> TokenStream {
    interned::interned(args, input)
}

#[proc_macro_derive(Supertype)]
pub fn supertype(input: TokenStream) -> TokenStream {
    supertype::supertype(input)
}

/// Creates input structs.
///
/// **Input structs** are the starting point of your program. Everything else in your program is
/// a deterministic function of these inputs.
///
/// # Container attributes
///
/// - `debug`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
/// - `singleton`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `data = <ident>`: TODO
/// - `constructor = <ident>`: Name of the constructor function. (default: `new`)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `persist([serialize = <path>], [deserialize = <path>])` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
/// - `default`: Marks the field as optional and as having a [`Default`] implementation.
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `no_eq`: Always mark the field as updated when its setter is called. The type does not have to implement [`Eq`].
/// - `get`: Name of the getter function (default: field name)
/// - `set`: Name of the setter function (default: `set_` + field name)
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
///
/// #[salsa::input]
/// struct File {
// FIXME: Doesn't work without the std::path:: prefix...
///     path: std::path::PathBuf,
///     #[returns(ref)]
///     contents: String,
/// }
///
/// #[salsa::input(singleton, debug)]
/// struct MySingleton {
///     field: u32,
/// }
/// ```
#[proc_macro_attribute]
pub fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    input::input(args, input)
}

/// Creates tracked structs, functions and [`impl`]s.
///
/// # Tracked structs
///
/// **Tracked structs** are usually used as parameters to tracked functions. They can only be created inside tracked functions.
///
/// ## Container attributes
///
/// - `debug`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
/// - `singleton`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `data = <ident>`: TODO
/// - `constructor = <ident>`: Name of the constructor function. (default: `new`)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `persist([serialize = <path>], [deserialize = <path>])` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// ## Field attributes
///
/// - `tracked`: Marks the field as tracked. Fields without this attribute must implement [`Hash`](std::hash::Hash).
///   * Modifications to tracked fields only invalidates the data depending on the tracked fields. Use tracked fields when you need fine-grained incremental recomputation.
///   * Modifications to untracked fields invalidates everything depending on the whole tracked struct. Use untracked fields for identity-defining data that rarely changes.
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `no_eq`: Always mark the field as updated when the struct is recreated inside a tracked function. The type does not have to implement [`Eq`].
/// - `get(<ident>)`: Name of the getter function (default: field name)
// Explicitly not documented: - `maybe_update(<expr>)`: Function of type `unsafe fn(*mut #field_ty, #field_ty) -> bool`. TODO
///
/// # Tracked functions
///
/// When you call a **tracked function**, Salsa will track which queries it runs and memoize the return value based on it. This data is saved in the database. When the function is called again and there is no matching memoized value (e.g. when inputs change), the queries are re-run and their outputs are compared. If they're identical, the first output is returned.
///
/// Tracked functions always take the database as the first argument and can take ingredients for the rest of the inputs.
///
/// **Ingredients** are structs tagged with [`#[input]`](fn@input), [`#[tracked]`](fn@tracked),
/// [`#[interned]`](fn@interned) and [`#[accumulator]`](fn@accumulator).
/// **Queries** are getters of ingredients' fields (like `input.value(db)` in the example below) or
/// other tracked functions.
///
/// ## Attributes
///
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `specify`: Indicate that the value can be externally specified (only works with a single Salsa struct as the input. Incompatible with `lru`)
/// - `no_eq`: Always mark the output as updated when function is re-created. The type does not have to implement [`Eq`]. This is incompatible with `cycle_fn`.
// Explicitly not documented: - `unsafe(non_update_return_type)`
/// - `cycle_fn = <path>`: Cycle recovery function. TODO
/// - `cycle_initial = <path>`: Initial value for cycle iteration. TODO
/// - `cycle_result = <expr>`: Result for non-fixpoint cycle. TODO
/// - `lru = <usize>`: Set to a nonzero value to enable LRU (Least Recently Used) eviction of memoized values and set the LRU capacity. (default: 0)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `self_ty = <type>`: Set the self type of the tracked impl, merely to refine the query name.
/// - `persist` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///
/// # Tracked [`impl`]s
///
/// TODO
///
/// # Examples
///
/// ```
/// #[salsa::tracked]
/// struct MyTracked<'db> {
///     value: u32,
///     #[returns(ref)]
///     links: Vec<MyTracked<'db>>,
/// }
///
/// #[salsa::tracked]
/// fn sum<'db>(db: &'db dyn salsa::Database, input: MyTracked<'db>) -> u32 {
///     input.value(db)
///         + input
///             .links(db)
///             .iter()
///             .map(|&file| sum(db, file))
///             .sum::<u32>()
/// }
///
/// ```
///
/// ```
/// //! Comparison between tracked and untracked fields.
///
/// #[salsa::tracked]
/// struct MyStruct<'db> {
///     #[tracked]
///     tracked_field: u32,
///     untracked_field: String, // No #[tracked] attribute
/// }
///
/// // If untracked_field changes, both functions re-execute
/// #[salsa::tracked]
/// fn uses_tracked<'db>(db: &'db dyn salsa::Database, s: MyStruct<'db>) -> u32 {
///     s.tracked_field(db)
/// }
/// #[salsa::tracked]
/// fn uses_untracked<'db>(db: &'db dyn salsa::Database, s: MyStruct<'db>) -> String {
///     s.untracked_field(db)
/// }
/// ```
///
/// [`impl`]: https://doc.rust-lang.org/std/keyword.impl.html
#[proc_macro_attribute]
pub fn tracked(args: TokenStream, input: TokenStream) -> TokenStream {
    tracked::tracked(args, input)
}

#[proc_macro_derive(Update, attributes(update))]
pub fn update(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::DeriveInput);
    match update::update_derive(item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

pub(crate) fn token_stream_with_error(mut tokens: TokenStream, error: syn::Error) -> TokenStream {
    tokens.extend(TokenStream::from(error.into_compile_error()));
    tokens
}

mod kw {
    syn::custom_keyword!(with);
    syn::custom_keyword!(maybe_update);
}
