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

#[proc_macro_attribute]
pub fn accumulator(args: TokenStream, input: TokenStream) -> TokenStream {
    accumulator::accumulator(args, input)
}

/// Implements a custom database trait.
///
/// Apply this on a custom database trait's definition and the `struct` and `impl` items of
/// implementors.
///
/// When applied to `struct` items, this macro implements the necessary supertraits required for `salsa::Database`.
///
/// When applied to `trait` and `impl` items, this macro adds some hidden trait methods required for [`#[tracked]`](fn@tracked) functions.
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
#[proc_macro_attribute]
pub fn db(args: TokenStream, input: TokenStream) -> TokenStream {
    db::db(args, input)
}

/// Creates interned structs.
///
/// **Container options:**
///
/// - TODO
///
/// **Field options:**
///
/// - TODO
///
/// # Example
///
/// ```
/// #[salsa::interned]
/// struct MyInterned<'db> {
///     field: String,
/// }
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
/// **Container options:**
///
/// - TODO
///
/// **Field options:**
///
/// - `default`: Marks the field as tracked.
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `no_eq`: Signal that the output type does not implement the `Eq` trait (incompatible with `cycle_fn`)
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
// Doesn't work without the std::path:: prefix...
///     path: std::path::PathBuf,
///     #[returns(ref)]
///     contents: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    input::input(args, input)
}

/// Creates tracked structs, functions and `impl`s.
///
/// # Tracked structs
///
/// **Container options:**
///
/// - `debug`
/// - `singleton`
/// - `data`
/// - `constructor_name`
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Fields) -> usize`, default: none)
/// - `persist(serialize = <path>, deserialize = <path>)` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn()`
///   * Type of `deserialize`: `fn()`
///
/// **Field options:**
///
/// - `tracked`: Marks the field as tracked.
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `no_eq`: Signal that the output type does not implement the `Eq` trait (incompatible with `cycle_fn`)
/// - `get`: Name of the getter function (default: field name)
/// - `maybe_update`: TODO
///
/// # Tracked functions
///
/// When you call a tracked function, Salsa will track which inputs it accesses and memoize the return value based on it. This data is saved in the database. When it's called again, the inputs are compared. If they're identical, the first.
///
/// Tracked functions always take the database as the first argument and can take [`#[input]`](fn@input), [`#[tracked]`](fn@tracked), [`#[interned]`](fn@interned) and [`#[accumulator]`](fn@accumulator) structs for the rest.
/// arguments.
///
/// **Options:**
///
/// - `returns(copy | clone | ref | deref | as_ref | as_deref)`: Configure the "return mode" (default: `clone`)
/// - `specify`: Signal that the value can be externally specified (only works with a single Salsa struct as the input. incompatible with `lru`)
/// - `no_eq`: Signal that the output type does not implement the `Eq` trait (incompatible with `cycle_fn`)
// Explicitly not documented: - `unsafe(non_update_return_type)`
/// - `cycle_fn = <path>`: TODO
/// - `cycle_initial = <path>`: TODO
/// - `cycle_result = <path>`: TODO
/// - `lru = <usize>`: Set the LRU capacity (default: 0)
/// - `heap_size = <path>`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `self_ty = <Ty>`: Set the self type of the tracked impl, merely to refine the query name
/// - `persist` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///
/// # Tracked `impl`s
///
/// TODO
///
/// # Example
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
