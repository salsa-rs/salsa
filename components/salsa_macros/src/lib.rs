//! This crate provides salsa's macros and attributes.

#![recursion_limit = "128"]

extern crate proc_macro;
extern crate proc_macro2;
#[macro_use]
extern crate quote;

use heck::CamelCase;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::ToTokens;
use syn::{parse_macro_input, AttributeArgs, FnArg, Ident, ItemTrait, ReturnType, TraitItem};

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
///     #[salsa::XXX] // see below for legal attributes
///     fn my_query(&self, input: u32) -> u64;
///
///     /// Queries can have any number of inputs (including zero); if there
///     /// is not exactly one input, then the key type will be
///     /// a tuple of the input types, so in this case `(u32, f32)`.
///     fn other_query(input1: u32, input2: f32) -> u64;
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
/// ```ignore
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
/// ```compile-fail
/// #[salsa::query_group]
/// trait CodegenDatabase {
///     #[salsa::input]
///     #[salsa::invoke(typeck::my_query)]
///     fn my_query(&self, input: u32) -> u64;
/// }
/// ```
#[proc_macro_attribute]
pub fn query_group(args: TokenStream, input: TokenStream) -> TokenStream {
    let _args = parse_macro_input!(args as AttributeArgs);
    let input = parse_macro_input!(input as ItemTrait);
    // println!("args: {:#?}", args);
    // println!("input: {:#?}", input);

    let trait_vis = input.vis;
    let trait_name = input.ident;
    let _generics = input.generics.clone();

    // Decompose the trait into the corresponding queries.
    let mut queries = vec![];
    for item in input.items {
        match item {
            TraitItem::Method(method) => {
                let mut storage = QueryStorage::Memoized;
                let mut invoke = None;
                let mut query_type = Ident::new(
                    &format!("{}Query", method.sig.ident.to_string().to_camel_case()),
                    Span::call_site(),
                );
                let mut num_storages = 0;

                // Extract attributes.
                let mut attrs = vec![];
                for attr in method.attrs {
                    // Leave non-salsa attributes untouched. These are
                    // attributes that don't start with `salsa::` or don't have
                    // exactly two segments in their path.
                    if is_salsa_attr_path(&attr.path) {
                        attrs.push(attr);
                        continue;
                    }

                    // Keep the salsa attributes around.
                    let name = attr.path.segments[1].ident.to_string();
                    let tts = attr.tts.into();
                    match name.as_str() {
                        "memoized" => {
                            storage = QueryStorage::Memoized;
                            num_storages += 1;
                        }
                        "volatile" => {
                            storage = QueryStorage::Volatile;
                            num_storages += 1;
                        }
                        "dependencies" => {
                            storage = QueryStorage::Dependencies;
                            num_storages += 1;
                        }
                        "input" => {
                            storage = QueryStorage::Input;
                            num_storages += 1;
                        }
                        "invoke" => {
                            invoke = Some(parse_macro_input!(tts as Parenthesized<syn::Path>).0);
                        }
                        "query_type" => {
                            query_type = parse_macro_input!(tts as Parenthesized<Ident>).0;
                        }
                        _ => panic!("unknown salsa attribute `{}`", name),
                    }
                }

                // Check attribute combinations.
                if num_storages > 1 {
                    panic!("multiple storage attributes specified");
                }
                if invoke.is_some() && storage == QueryStorage::Input {
                    panic!("#[salsa::invoke] cannot be set on #[salsa::input] queries");
                }

                // Extract keys.
                let mut iter = method.sig.decl.inputs.iter();
                match iter.next() {
                    Some(FnArg::SelfRef(sr)) if sr.mutability.is_none() => (),
                    _ => panic!(
                        "first argument of query `{}` must be `&self` or `&mut self`",
                        method.sig.ident
                    ),
                }
                let mut keys = vec![];
                for arg in iter {
                    match *arg {
                        FnArg::Captured(ref arg) => {
                            keys.push(arg.ty.clone());
                        }
                        ref a => panic!("unsupported argument `{:?}` of `{}`", a, method.sig.ident),
                    }
                }

                // Extract value.
                let value = match method.sig.decl.output {
                    ReturnType::Type(_, ref ty) => ty.as_ref().clone(),
                    ref r => panic!(
                        "unsupported return type `{:?}` of `{}`",
                        r, method.sig.ident
                    ),
                };

                queries.push(Query {
                    query_type,
                    fn_name: method.sig.ident.clone(),
                    attrs,
                    storage,
                    keys,
                    value,
                    invoke,
                });
            }
            _ => (),
        }
    }

    let mut query_fn_declarations = proc_macro2::TokenStream::new();
    let mut query_fn_definitions = proc_macro2::TokenStream::new();
    for query in &queries {
        let key_names: &Vec<_> = &(0..query.keys.len())
            .map(|i| Ident::new(&format!("key{}", i), Span::call_site()))
            .collect();
        let keys = &query.keys;
        let value = &query.value;
        let fn_name = &query.fn_name;
        let qt = &query.query_type;
        let attrs = &query.attrs;

        query_fn_declarations.extend(quote! {
            #(#attrs)*
            fn #fn_name(&self, #(#key_names: #keys),*) -> #value;
        });

        query_fn_definitions.extend(quote! {
            fn #fn_name(&self, #(#key_names: #keys),*) -> #value {
                <Self as salsa::plumbing::GetQueryTable<#qt>>::get_query_table(self).get((#(#key_names),*))
            }
        });
    }

    // Emit the trait itself.
    let mut output = {
        let attrs = &input.attrs;
        let qts = queries.iter().map(|q| &q.query_type);
        let bounds = &input.supertraits;
        quote! {
            #(#attrs)*
            #trait_vis trait #trait_name : #(salsa::plumbing::GetQueryTable<#qts> +)* #bounds {
                #query_fn_declarations
            }
        }
    };

    // Emit an impl of the trait
    output.extend({
        let qts = queries.iter().map(|q| &q.query_type);
        let bounds = &input.supertraits;
        quote! {
            impl<T> #trait_name for T
            where
                T: #(salsa::plumbing::GetQueryTable<#qts> +)* #bounds
            {
                #query_fn_definitions
            }
        }
    });

    // Emit the query types.
    for query in queries {
        let qt = &query.query_type;
        let storage = Ident::new(
            match query.storage {
                QueryStorage::Memoized => "MemoizedStorage",
                QueryStorage::Volatile => "VolatileStorage",
                QueryStorage::Dependencies => "DependencyStorage",
                QueryStorage::Input => "InputStorage",
            },
            Span::call_site(),
        );
        let keys = &query.keys;
        let value = &query.value;

        // Emit the query struct and implement the Query trait on it.
        output.extend(quote! {
            #[derive(Default, Debug)]
            #trait_vis struct #qt;

            impl<DB> salsa::Query<DB> for #qt
            where
                DB: #trait_name,
            {
                type Key = (#(#keys),*);
                type Value = #value;
                type Storage = salsa::plumbing::#storage<DB, Self>;
            }
        });

        // Implement the QueryFunction trait for all queries except inputs.
        if query.storage != QueryStorage::Input {
            let span = query.fn_name.span();
            let key_names: &Vec<_> = &(0..query.keys.len())
                .map(|i| Ident::new(&format!("key{}", i), Span::call_site()))
                .collect();
            let key_pattern = if query.keys.len() == 1 {
                quote! { #(#key_names),* }
            } else {
                quote! { (#(#key_names),*) }
            };
            let invoke = match &query.invoke {
                Some(i) => i.into_token_stream(),
                None => query.fn_name.into_token_stream(),
            };
            output.extend(quote_spanned! {span=>
                impl<DB> salsa::plumbing::QueryFunction<DB> for #qt
                where
                    DB: #trait_name,
                {
                    fn execute(db: &DB, #key_pattern: <Self as salsa::Query<DB>>::Key)
                        -> <Self as salsa::Query<DB>>::Value {
                        #invoke(db, #(#key_names),*)
                    }
                }
            });
        }
    }

    output.into()
}

fn is_salsa_attr_path(path: &syn::Path) -> bool {
    path.segments
        .first()
        .map(|s| s.value().ident != "salsa")
        .unwrap_or(true)
        || path.segments.len() != 2
}

#[derive(Debug)]
struct Query {
    fn_name: Ident,
    attrs: Vec<syn::Attribute>,
    query_type: Ident,
    storage: QueryStorage,
    keys: Vec<syn::Type>,
    value: syn::Type,
    invoke: Option<syn::Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryStorage {
    Memoized,
    Volatile,
    Dependencies,
    Input,
}

struct Parenthesized<T>(pub T);

impl<T> syn::parse::Parse for Parenthesized<T>
where
    T: syn::parse::Parse,
{
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let content;
        syn::parenthesized!(content in input);
        content.parse::<T>().map(Parenthesized)
    }
}
