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
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
/// - `#[specify]`: Indicate that the value can be externally specified (only works with a single Salsa struct as the input. Incompatible with `lru`)
// For functions:
/// - `#[no_eq]`: Always mark the output as updated when function is re-created. The type does not have to implement [`Eq`]. This is incompatible with `cycle_fn`.
/// - `#[debug]`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
// Explicitly not documented due to deprecation: - `#[no_lifetime]`: TODO
// Explicitly not documented: - `#[unsafe(non_update_return_type)]`
/// - `#[singleton]`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `#[data = <ident>]`: Name of the data type for an interned struct.
// Explicitly not documented as it's unused: - `#[db = <path>]`: Path to the database.
// For functions:
/// - `#[cycle_fn = <path>]`: Cycle recovery function, invoked on each iteration of a fixpoint cycle.
///   Signature: `fn(&Db, &salsa::Cycle, &Output, Output, Input) -> Output`.
///   Receives the database, the cycle state (including the iteration count), the previous
///   provisional value, the newly computed value, and the query input. If the returned value
///   equals `last_provisional_value`, the cycle has converged and iteration stops.
///   (default: panics on cycle with `salsa::plumbing::unexpected_cycle_recovery!`)
// For functions:
/// - `#[cycle_initial = <path>]`: Initial value to seed fixpoint iteration when a cycle is first detected.
///   Signature: `fn(&Db, salsa::Id, Input) -> Output`.
///   This value is returned as a provisional result while the cycle resolves. It should be
///   a reasonable starting point (e.g., an empty/default/identity value).
///   (default: `salsa::plumbing::unexpected_cycle_initial!`)
// For functions:
/// - `#[cycle_result = <expr>]`: Fallback value for immediate (non-iterative) cycle recovery.
///   When set without `cycle_fn`, the cycle head returns this fallback immediately instead
///   of iterating. Use when you have a sentinel value and don't need convergence.
///   Signature: `fn(&Db, salsa::Id) -> Output`.
///   Mutually exclusive with `cycle_fn` and `cycle_initial`.
/// - `#[lru = <usize>]`: Set to a nonzero value to enable LRU (Least Recently Used) eviction of memoized values and set the LRU capacity. (default: 0)
/// - `#[constructor = <ident>]`: Name of the constructor function. (default: `new`)
// Explicitly not documented: - `#[id = <path>]`: custom ID for interned structs. Must implement `salsa::plumbing::AsId`. (default: `salsa::Id`)
/// - `#[revisions = <expr as usize>]`: minimum number of revisions to keep a value interned.
///   (default: `salsa::plumbing::internal::Configuration::REVISIONS`)
/// - `#[heap_size = <path>]`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `#[self_ty = <type>]`: Set the self type of the tracked impl, merely to refine the query name.
/// - `#[persist]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
/// - `#[persist([serialize = <path>], [deserialize = <path>])]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_TRACKED`]:
/// - `#[tracked]`: Marks the field as tracked. Fields without this attribute must implement [`Hash`](std::hash::Hash).
///   * Modifications to tracked fields only invalidates the data depending on the tracked fields. Use tracked fields when you need fine-grained incremental recomputation.
///   * Modifications to untracked fields invalidates everything depending on the whole tracked struct. Use untracked fields for identity-defining data that rarely changes.
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_DEFAULT`]:
/// - `#[default]`: Marks the field as optional and as having a [`Default`] implementation.
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
// For input structs:
/// - `#[no_eq]`: Always mark the field as updated when its setter is called. The type does not have to implement [`Eq`].
// For tracked structs:
/// - `#[no_eq]`: Always mark the field as updated when the struct is recreated inside a tracked function. The type does not have to implement [`Eq`].
/// - `#[get(<ident>)]`: Name of the getter function (default: field name)
// Only for inputs:
/// - `#[set(<ident>)]`: Name of the setter function (default: `set_` + field name)
// Only if [`salsa_struct::SalsaStructAllowedOptions::ALLOW_MAYBE_UPDATE`]:
// Explicitly not documented: - `#[maybe_update(<expr>)]`: Function of type `unsafe fn(*mut #field_ty, #field_ty) -> bool`. TODO
mod attrs_doc {}

/// Creates an accumulator struct.
///
/// **Accumulators** collect values during tracked function execution. Inside a tracked
/// function, call `.accumulate(db)` on an accumulator value to store it. After the
/// tracked function completes, accumulated values can be retrieved for a given input
/// with `<AccumulatorType>::accumulated(db, input)`.
///
/// The struct must have a single unnamed field containing the data type to accumulate.
///
/// Accumulators integrate with Salsa's incremental recomputation: when a tracked function
/// re-executes, its old accumulated values are replaced with the new ones. If a tracked
/// function is not re-executed (because its inputs haven't changed), its accumulated
/// values from the previous revision are retained.
///
/// This macro accepts no options.
///
/// # Example
///
/// ```
/// use salsa::Accumulator;
///
/// #[salsa::accumulator]
/// #[derive(Debug)]
/// struct Log(String);
///
/// #[salsa::tracked]
/// fn my_fn(db: &dyn salsa::Database) {
///     Log("something happened".to_string()).accumulate(db);
/// }
/// ```
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

/// Creates an interned struct.
///
/// **Interned structs** are dedpulicated, immutable structs used as parameters to tracked
/// functions.
///
/// # Container attributes
///
/// - `#[debug]`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
// Explicitly not documented due to deprecation: - `#[no_lifetime]`: TODO
/// - `#[singleton]`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `#[data = <ident>]`: TODO
/// - `#[constructor = <ident>]`: Name of the constructor function. (default: `new`)
// Explicitly not documented: - `#[id = <path>]`: TODO (default: `salsa::Id`)
/// - `#[revisions = <expr as usize>]`: minimum number of revisions to keep a value interned.
///   (default: `salsa::plumbing::internal::Configuration::REVISIONS`)
/// - `#[heap_size = <path>]`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `#[persist([serialize = <path>], [deserialize = <path>])]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
/// - `#[get(<ident>)]`: Name of the getter function (default: field name)
///
/// # Example
///
/// ```
/// #[salsa::interned(debug)]
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

/// Generates forwarding impls so an enum can be used as a Salsa struct parameter.
///
/// Apply this derive to an enum whose variants each wrap a single Salsa struct
/// (e.g., `#[salsa::input]`, `#[salsa::interned]`, or `#[salsa::tracked]`).
/// It generates forwarding implementations of `AsId`, `FromIdWithDb`, and
/// `SalsaStructInDb` that delegate to the variant's inner type.
///
/// This allows a tracked function to accept the enum as an argument, dispatching
/// to the appropriate variant at runtime.
///
/// Supertypes can be nested: a variant can wrap another supertype enum.
///
/// This derive accepts no options.
///
/// # Example
///
/// ```
/// #[salsa::interned(debug)]
/// struct Name<'db> { name: String }
///
/// #[salsa::interned(debug)]
/// struct Id<'db> { id: u32 }
///
/// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
/// enum Person<'db> {
///     Name(Name<'db>),
///     Id(Id<'db>),
/// }
///
/// #[salsa::tracked]
/// fn describe(db: &dyn salsa::Database, person: Person<'_>) -> String {
///     match person {
///         Person::Name(name) => name.name(db),
///         Person::Id(id) => id.id(db).to_string(),
///     }
/// }
///
/// let db = salsa::DatabaseImpl::new();
/// let name = Name::new(&db, "example");
/// let id = Id::new(&db, 42);
///
/// let person_a = Person::Name(name);
/// let person_b = Person::Id(id);
///
/// assert_eq!(describe(&db, person_a), "example");
/// assert_eq!(describe(&db, person_b), "42");
/// ```
#[proc_macro_derive(Supertype)]
pub fn supertype(input: TokenStream) -> TokenStream {
    supertype::supertype(input)
}

/// Creates an input struct.
///
/// **Input structs** are the starting point of your program. Everything else in your program is
/// a deterministic function of these inputs.
///
/// # Container attributes
///
/// - `#[debug]`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
/// - `#[singleton]`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `#[data = <ident>]`: TODO
/// - `#[constructor = <ident>]`: Name of the constructor function. (default: `new`)
/// - `#[heap_size = <path>]`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `#[persist([serialize = <path>], [deserialize = <path>])]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// # Field attributes
///
/// - `#[default]`: Marks the field as optional and as having a [`Default`] implementation.
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
/// - `#[no_eq]`: Always mark the field as updated when its setter is called. The type does not have to implement [`Eq`].
/// - `#[get]`: Name of the getter function (default: field name)
/// - `#[set]`: Name of the setter function (default: `set_` + field name)
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
///
/// let db = salsa::DatabaseImpl::new();
/// let a = MySingleton::new(&db, 1);
/// let b = MySingleton::get(&db);
///
/// assert_eq!(a.field(&db), b.field(&db));
/// ```
///
/// ```should_panic
/// # #[salsa::input(singleton, debug)]
/// # struct MySingleton {
/// #     field: u32,
/// # }
/// // Defining two instances of a singleton will panic.
/// let db = salsa::DatabaseImpl::new();
/// let a = MySingleton::new(&db, 1);
/// let b = MySingleton::new(&db, 2);
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
/// - `#[debug]`: Generate a [`Debug`](std::fmt::Debug) implementation for the struct.
/// - `#[singleton]`: Marks the struct as a singleton. There is a maximum of one instance of a singleton struct in a Salsa database. Singletons additionally have `get` and `try_get` methods, and their `new` method sets the singleton.
// Explicitly not documented as it's unused: - `#[data = <ident>]`: TODO
/// - `#[constructor = <ident>]`: Name of the constructor function. (default: `new`)
/// - `#[heap_size = <path>]`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `#[persist([serialize = <path>], [deserialize = <path>])]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///   * Type of `serialize`: `fn(&Fields<'_>, S) -> Result<S::Ok, S::Error> where S: serde::Serializer`
///   * Type of `deserialize`: `fn(D) -> Result<Fields<'static>, D::Error> where D: serde::Deserializer<'de>`
///
/// ## Field attributes
///
/// - `#[tracked]`: Marks the field as tracked. Fields without this attribute must implement [`Hash`](std::hash::Hash).
///   * Modifications to tracked fields only invalidates the data depending on the tracked fields. Use tracked fields when you need fine-grained incremental recomputation.
///   * Modifications to untracked fields invalidates everything depending on the whole tracked struct. Use untracked fields for identity-defining data that rarely changes.
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
/// - `#[no_eq]`: Always mark the field as updated when the struct is recreated inside a tracked function. The type does not have to implement [`Eq`].
/// - `#[get(<ident>)]`: Name of the getter function (default: field name)
// Explicitly not documented: - `#[maybe_update(<expr>)]`: Function of type `unsafe fn(*mut #field_ty, #field_ty) -> bool`. TODO
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
/// - `#[returns(copy | clone | ref | deref | as_ref | as_deref)]`: Configure the "return mode" (default: `clone`)
/// - `#[specify]`: Indicate that the value can be externally specified (only works with a single Salsa struct as the input. Incompatible with `lru`)
/// - `#[no_eq]`: Always mark the output as updated when function is re-created. The type does not have to implement [`Eq`]. This is incompatible with `cycle_fn`.
// Explicitly not documented: - `#[unsafe(non_update_return_type)]`
/// - `#[cycle_fn = <path>]`: Cycle recovery function, invoked on each iteration of a fixpoint cycle.
///   Signature: `fn(&Db, &salsa::Cycle, &Output, Output, Input) -> Output`.
///   Receives the database, the cycle state (including the iteration count), the previous
///   provisional value, the newly computed value, and the query input. If the returned value
///   equals `last_provisional_value`, the cycle has converged and iteration stops.
///   (default: panics on cycle with `salsa::plumbing::unexpected_cycle_recovery!`)
/// - `#[cycle_initial = <path>]`: Initial value to seed fixpoint iteration when a cycle is first detected.
///   Signature: `fn(&Db, salsa::Id, Input) -> Output`.
///   This value is returned as a provisional result while the cycle resolves. It should be
///   a reasonable starting point (e.g., an empty/default/identity value).
///   (default: `salsa::plumbing::unexpected_cycle_initial!`)
/// - `#[cycle_result = <expr>]`: Fallback value for immediate (non-iterative) cycle recovery.
///   When set without `cycle_fn`, the cycle head returns this fallback immediately instead
///   of iterating. Use when you have a sentinel value and don't need convergence.
///   Signature: `fn(&Db, salsa::Id) -> Output`.
///   Mutually exclusive with `cycle_fn` and `cycle_initial`.
/// - `#[lru = <usize>]`: Set to a nonzero value to enable LRU (Least Recently Used) eviction of memoized values and set the LRU capacity. (default: 0)
/// - `#[heap_size = <path>]`: Function to calculate the heap memory usage of memoized values (type: `fn(&Output) -> usize`, default: none)
/// - `#[self_ty = <type>]`: Set the self type of the tracked impl, merely to refine the query name.
/// - `#[persist]` (Only with <span class="stab portability"><code>persistence</code></span> feature)
///
/// # Tracked [`impl`]s
///
/// `#[salsa::tracked]` can be applied to an `impl` block to associate tracked functions with
/// a specific type.
///
/// The `impl` block may be inherent or a trait impl. Each tracked method or associated function
/// must be annotated with `#[salsa::tracked]`.
///
/// When `self` is used, it precedes the database argument and must be taken by value (`self`, not
/// `&self` or `&mut self`).
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
/// ```
/// //! Associated function on a plain struct (used as a namespace)
///
/// #[salsa::input]
/// struct MyInput {
///     field: u32,
/// }
///
/// struct MyModule;
///
/// #[salsa::tracked]
/// impl MyModule {
///     #[salsa::tracked]
///     fn compute(db: &dyn salsa::Database, input: MyInput) -> u32 {
///         input.field(db) * 2
///     }
/// }
///
/// let db = salsa::DatabaseImpl::new();
/// let input = MyInput::new(&db, 21);
/// assert_eq!(MyModule::compute(&db, input), 42);
/// ```
///
/// ```
/// //! Method on a Salsa struct
///
/// #[salsa::input]
/// struct MyInput {
///     field: u32,
/// }
///
/// #[salsa::tracked]
/// impl MyInput {
///     #[salsa::tracked]
///     fn doubled(self, db: &dyn salsa::Database) -> u32 {
///         self.field(db) * 2
///     }
/// }
///
/// let db = salsa::DatabaseImpl::new();
/// let input = MyInput::new(&db, 21);
/// assert_eq!(input.doubled(&db), 42);
/// ```
///
/// ```
/// //! Trait implementation on a Salsa struct
///
/// trait MyTrait {
///     fn describe(self, db: &dyn salsa::Database) -> String;
/// }
///
/// #[salsa::input]
/// struct MyInput {
///     name: String,
/// }
///
/// #[salsa::tracked]
/// impl MyTrait for MyInput {
///     #[salsa::tracked]
///     fn describe(self, db: &dyn salsa::Database) -> String {
///         self.name(db)
///     }
/// }
///
/// let db = salsa::DatabaseImpl::new();
/// let input = MyInput::new(&db, "hello".to_string());
/// assert_eq!(input.describe(&db), "hello");
/// ```
///
/// [`impl`]: https://doc.rust-lang.org/std/keyword.impl.html
#[proc_macro_attribute]
pub fn tracked(args: TokenStream, input: TokenStream) -> TokenStream {
    tracked::tracked(args, input)
}

/// Derives the [`salsa::Update`] trait for a type.
///
/// `salsa::Update` enables in-place mutation of values across Salsa revisions. Instead of replacing an
/// entire value when a struct is re-created in a newer revision, Salsa calls
/// `salsa::Update::maybe_update` on each field, preserving heap allocations and avoiding unnecessary
/// invalidation of downstream queries that only depend on unchanged fields.
///
/// The derive recursively calls `salsa::Update::maybe_update` on each field. For fields whose type does
/// not implement `Update`, it falls back to [`PartialEq`]-based comparison: the field is
/// overwritten only if the old and new values differ.
///
/// # Field attributes
///
/// - `#[update(unsafe(with(function)))]`: Use `function` instead of the default
///   `salsa::Update::maybe_update`. The function must have the signature
///   `unsafe fn(*mut FieldType, FieldType) -> bool`.
///
/// # Example
///
/// ```
/// #[derive(salsa::Update)]
/// struct MyData {
///     value: u32,
///     items: Vec<String>,
/// }
///
/// #[derive(salsa::Update)]
/// struct CustomField {
///     #[update(unsafe(with(my_update_fn)))]
///     data: Vec<u8>,
/// }
///
/// unsafe fn my_update_fn(old: *mut Vec<u8>, new: Vec<u8>) -> bool {
///     // custom update logic
///     unsafe { &mut *old }.clear();
///     unsafe { &mut *old }.extend(new);
///     true
/// }
/// ```
///
/// # Safety
///
/// The generated impl delegates to [`Update::maybe_update`] and inherits its safety requirements:
/// `old_pointer` must point to a valid-but-potentially-stale value from a prior revision, and
/// borrowed data within it may be dangling.
///
/// [`Update`]: crate::Update
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
