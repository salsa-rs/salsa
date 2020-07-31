use std::convert::TryFrom;

use crate::parenthesized::Parenthesized;
use heck::CamelCase;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::ToTokens;
use syn::{
    parse_macro_input, parse_quote, spanned::Spanned, Attribute, Error, FnArg, Ident, ItemTrait,
    ReturnType, TraitItem, Type,
};

/// Implementation for `[salsa::query_group]` decorator.
pub(crate) fn query_group(args: TokenStream, input: TokenStream) -> TokenStream {
    let group_struct = parse_macro_input!(args as Ident);
    let input: ItemTrait = parse_macro_input!(input as ItemTrait);
    // println!("args: {:#?}", args);
    // println!("input: {:#?}", input);

    let input_span = input.span();
    let (trait_attrs, salsa_attrs) = filter_attrs(input.attrs);
    if !salsa_attrs.is_empty() {
        return Error::new(
            input_span,
            format!("unsupported attributes: {:?}", salsa_attrs),
        )
        .to_compile_error()
        .into();
    }

    let trait_vis = input.vis;
    let trait_name = input.ident;
    let _generics = input.generics.clone();
    let dyn_db = quote! { dyn #trait_name };

    // Decompose the trait into the corresponding queries.
    let mut queries = vec![];
    for item in input.items {
        match item {
            TraitItem::Method(method) => {
                let mut storage = QueryStorage::Memoized;
                let mut cycle = None;
                let mut invoke = None;
                let query_name = method.sig.ident.to_string();
                let mut query_type = Ident::new(
                    &format!("{}Query", method.sig.ident.to_string().to_camel_case()),
                    Span::call_site(),
                );
                let mut num_storages = 0;

                // Extract attributes.
                let (attrs, salsa_attrs) = filter_attrs(method.attrs);
                for SalsaAttr { name, tts, span } in salsa_attrs {
                    match name.as_str() {
                        "memoized" => {
                            storage = QueryStorage::Memoized;
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
                        "interned" => {
                            storage = QueryStorage::Interned;
                            num_storages += 1;
                        }
                        "cycle" => {
                            cycle = Some(parse_macro_input!(tts as Parenthesized<syn::Path>).0);
                        }
                        "invoke" => {
                            invoke = Some(parse_macro_input!(tts as Parenthesized<syn::Path>).0);
                        }
                        "query_type" => {
                            query_type = parse_macro_input!(tts as Parenthesized<Ident>).0;
                        }
                        "transparent" => {
                            storage = QueryStorage::Transparent;
                            num_storages += 1;
                        }
                        _ => {
                            return Error::new(span, format!("unknown salsa attribute `{}`", name))
                                .to_compile_error()
                                .into();
                        }
                    }
                }

                // Check attribute combinations.
                if num_storages > 1 {
                    return Error::new(method.sig.span(), "multiple storage attributes specified")
                        .to_compile_error()
                        .into();
                }
                match &invoke {
                    Some(invoke) if storage == QueryStorage::Input => {
                        return Error::new(
                            invoke.span(),
                            "#[salsa::invoke] cannot be set on #[salsa::input] queries",
                        )
                        .to_compile_error()
                        .into();
                    }
                    _ => {}
                }

                // Extract keys.
                let mut iter = method.sig.inputs.iter();
                match iter.next() {
                    Some(FnArg::Receiver(sr)) if sr.mutability.is_none() => (),
                    _ => {
                        return Error::new(
                            method.sig.span(),
                            format!(
                                "first argument of query `{}` must be `&self`",
                                method.sig.ident,
                            ),
                        )
                        .to_compile_error()
                        .into();
                    }
                }
                let mut keys: Vec<Type> = vec![];
                for arg in iter {
                    match *arg {
                        FnArg::Typed(ref arg) => {
                            keys.push((*arg.ty).clone());
                        }
                        ref arg => {
                            return Error::new(
                                arg.span(),
                                format!(
                                    "unsupported argument `{:?}` of `{}`",
                                    arg, method.sig.ident,
                                ),
                            )
                            .to_compile_error()
                            .into();
                        }
                    }
                }

                // Extract value.
                let value = match method.sig.output {
                    ReturnType::Type(_, ref ty) => ty.as_ref().clone(),
                    ref ret => {
                        return Error::new(
                            ret.span(),
                            format!(
                                "unsupported return type `{:?}` of `{}`",
                                ret, method.sig.ident
                            ),
                        )
                        .to_compile_error()
                        .into();
                    }
                };

                // For `#[salsa::interned]` keys, we create a "lookup key" automatically.
                //
                // For a query like:
                //
                //     fn foo(&self, x: Key1, y: Key2) -> u32
                //
                // we would create
                //
                //     fn lookup_foo(&self, x: u32) -> (Key1, Key2)
                let lookup_query = if let QueryStorage::Interned = storage {
                    let lookup_query_type = Ident::new(
                        &format!(
                            "{}LookupQuery",
                            method.sig.ident.to_string().to_camel_case()
                        ),
                        Span::call_site(),
                    );
                    let lookup_fn_name = Ident::new(
                        &format!("lookup_{}", method.sig.ident.to_string()),
                        method.sig.ident.span(),
                    );
                    let keys = &keys;
                    let lookup_value: Type = parse_quote!((#(#keys),*));
                    let lookup_keys = vec![value.clone()];
                    Some(Query {
                        query_type: lookup_query_type,
                        query_name: format!("lookup_{}", query_name),
                        fn_name: lookup_fn_name,
                        attrs: vec![], // FIXME -- some automatically generated docs on this method?
                        storage: QueryStorage::InternedLookup {
                            intern_query_type: query_type.clone(),
                        },
                        keys: lookup_keys,
                        value: lookup_value,
                        invoke: None,
                        cycle: cycle.clone(),
                    })
                } else {
                    None
                };

                queries.push(Query {
                    query_type,
                    query_name,
                    fn_name: method.sig.ident,
                    attrs,
                    storage,
                    keys,
                    value,
                    invoke,
                    cycle,
                });

                queries.extend(lookup_query);
            }
            _ => (),
        }
    }

    let group_storage = Ident::new(
        &format!("{}GroupStorage__", trait_name.to_string()),
        Span::call_site(),
    );

    let mut query_fn_declarations = proc_macro2::TokenStream::new();
    let mut query_fn_definitions = proc_macro2::TokenStream::new();
    let mut storage_fields = proc_macro2::TokenStream::new();
    let mut queries_with_storage = vec![];
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

        // Special case: transparent queries don't create actual storage,
        // just inline the definition
        if let QueryStorage::Transparent = query.storage {
            let invoke = query.invoke_tt();
            query_fn_definitions.extend(quote! {
                fn #fn_name(&self, #(#key_names: #keys),*) -> #value {
                    #invoke(self, #(#key_names),*)
                }
            });
            continue;
        }

        queries_with_storage.push(fn_name);

        query_fn_definitions.extend(quote! {
            fn #fn_name(&self, #(#key_names: #keys),*) -> #value {
                // Create a shim to force the code to be monomorphized in the
                // query crate. Our experiments revealed that this makes a big
                // difference in total compilation time in rust-analyzer, though
                // it's not totally obvious why that should be.
                fn __shim(db: &dyn #trait_name,  #(#key_names: #keys),*) -> #value {
                    salsa::plumbing::get_query_table::<#qt>(db).get((#(#key_names),*))
                }
                __shim(self, #(#key_names),*)

            }
        });

        // For input queries, we need `set_foo` etc
        if let QueryStorage::Input = query.storage {
            let set_fn_name = Ident::new(&format!("set_{}", fn_name), fn_name.span());
            let set_with_durability_fn_name =
                Ident::new(&format!("set_{}_with_durability", fn_name), fn_name.span());

            let set_fn_docs = format!(
                "
                Set the value of the `{fn_name}` input.

                See `{fn_name}` for details.

                *Note:* Setting values will trigger cancellation
                of any ongoing queries; this method blocks until
                those queries have been cancelled.
            ",
                fn_name = fn_name
            );

            let set_constant_fn_docs = format!(
                "
                Set the value of the `{fn_name}` input and promise
                that its value will never change again.

                See `{fn_name}` for details.

                *Note:* Setting values will trigger cancellation
                of any ongoing queries; this method blocks until
                those queries have been cancelled.
            ",
                fn_name = fn_name
            );

            query_fn_declarations.extend(quote! {
                # [doc = #set_fn_docs]
                fn #set_fn_name(&mut self, #(#key_names: #keys,)* value__: #value);


                # [doc = #set_constant_fn_docs]
                fn #set_with_durability_fn_name(&mut self, #(#key_names: #keys,)* value__: #value, durability__: salsa::Durability);
            });

            query_fn_definitions.extend(quote! {
                fn #set_fn_name(&mut self, #(#key_names: #keys,)* value__: #value) {
                    fn __shim(db: &mut dyn #trait_name, #(#key_names: #keys,)* value__: #value) {
                        salsa::plumbing::get_query_table_mut::<#qt>(db).set((#(#key_names),*), value__)
                    }
                    __shim(self, #(#key_names,)* value__)
                }

                fn #set_with_durability_fn_name(&mut self, #(#key_names: #keys,)* value__: #value, durability__: salsa::Durability) {
                    fn __shim(db: &mut dyn #trait_name, #(#key_names: #keys,)* value__: #value, durability__: salsa::Durability) {
                        salsa::plumbing::get_query_table_mut::<#qt>(db).set_with_durability((#(#key_names),*), value__, durability__)
                    }
                    __shim(self, #(#key_names,)* value__ ,durability__)
                }
            });
        }

        // A field for the storage struct
        //
        // FIXME(#120): the pub should not be necessary once we complete the transition
        storage_fields.extend(quote! {
            pub #fn_name: std::sync::Arc<<#qt as salsa::Query>::Storage>,
        });
    }

    // Emit the trait itself.
    let mut output = {
        let bounds = &input.supertraits;
        quote! {
            #(#trait_attrs)*
            #trait_vis trait #trait_name :
            salsa::Database +
            salsa::plumbing::HasQueryGroup<#group_struct> +
            #bounds
            {
                #query_fn_declarations
            }
        }
    };

    // Emit the query group struct and impl of `QueryGroup`.
    output.extend(quote! {
        /// Representative struct for the query group.
        #trait_vis struct #group_struct { }

        impl salsa::plumbing::QueryGroup for #group_struct
        {
            type DynDb = #dyn_db;
            type GroupStorage = #group_storage;
        }
    });

    // Emit an impl of the trait
    output.extend({
        let bounds = input.supertraits.clone();
        quote! {
            impl<DB> #trait_name for DB
            where
                DB: #bounds,
                DB: salsa::Database,
                DB: salsa::plumbing::HasQueryGroup<#group_struct>,
            {
                #query_fn_definitions
            }
        }
    });

    let non_transparent_queries = || {
        queries.iter().filter(|q| match q.storage {
            QueryStorage::Transparent => false,
            _ => true,
        })
    };

    // Emit the query types.
    for (query, query_index) in non_transparent_queries().zip(0_u16..) {
        let fn_name = &query.fn_name;
        let qt = &query.query_type;

        let storage = match &query.storage {
            QueryStorage::Memoized => quote!(salsa::plumbing::MemoizedStorage<Self>),
            QueryStorage::Dependencies => quote!(salsa::plumbing::DependencyStorage<Self>),
            QueryStorage::Input => quote!(salsa::plumbing::InputStorage<Self>),
            QueryStorage::Interned => quote!(salsa::plumbing::InternedStorage<Self>),
            QueryStorage::InternedLookup { intern_query_type } => {
                quote!(salsa::plumbing::LookupInternedStorage<Self, #intern_query_type>)
            }
            QueryStorage::Transparent => panic!("should have been filtered"),
        };
        let keys = &query.keys;
        let value = &query.value;
        let query_name = &query.query_name;

        // Emit the query struct and implement the Query trait on it.
        output.extend(quote! {
            #[derive(Default, Debug)]
            #trait_vis struct #qt;
        });

        output.extend(quote! {
            impl #qt {
                /// Get access to extra methods pertaining to this query. For
                /// example, you can use this to run the GC (`sweep`) across a
                /// single input. You can also use it to invoke this query, though
                /// it's more common to use the trait method on the database
                /// itself.
                #trait_vis fn in_db(self, db: &#dyn_db) -> salsa::QueryTable<'_, Self>
                {
                    salsa::plumbing::get_query_table::<#qt>(db)
                }
            }
        });

        if query.storage.supports_mut() {}
        output.extend(quote! {
            impl #qt {
                /// Like `in_db`, but gives access to methods for setting the
                /// value of an input. Not applicable to derived queries.
                ///
                /// # Threads, cancellation, and blocking
                ///
                /// Mutating the value of a query cannot be done while there are
                /// still other queries executing. If you are using your database
                /// within a single thread, this is not a problem: you only have
                /// `&self` access to the database, but this method requires `&mut
                /// self`.
                ///
                /// However, if you have used `snapshot` to create other threads,
                /// then attempts to `set` will **block the current thread** until
                /// those snapshots are dropped (usually when those threads
                /// complete). This also implies that if you create a snapshot but
                /// do not send it to another thread, then invoking `set` will
                /// deadlock.
                ///
                /// Before blocking, the thread that is attempting to `set` will
                /// also set a cancellation flag. In the threads operating on
                /// snapshots, you can use the [`is_current_revision_canceled`]
                /// method to check for this flag and bring those operations to a
                /// close, thus allowing the `set` to succeed. Ignoring this flag
                /// may lead to "starvation", meaning that the thread attempting
                /// to `set` has to wait a long, long time. =)
                ///
                /// [`is_current_revision_canceled`]: struct.Runtime.html#method.is_current_revision_canceled
                #trait_vis fn in_db_mut(self, db: &mut #dyn_db) -> salsa::QueryTableMut<'_, Self>
                {
                    salsa::plumbing::get_query_table_mut::<#qt>(db)
                }
            }

            // ANCHOR:Query_impl
            impl salsa::Query for #qt
            {
                type Key = (#(#keys),*);
                type Value = #value;
                type Storage = #storage;
                type Group = #group_struct;
                type GroupStorage = #group_storage;
                type DynDb = #dyn_db;

                const QUERY_INDEX: u16 = #query_index;

                const QUERY_NAME: &'static str = #query_name;

                fn query_storage(
                    group_storage: &Self::GroupStorage,
                ) -> &std::sync::Arc<Self::Storage> {
                    &group_storage.#fn_name
                }
            }
            // ANCHOR_END:Query_impl
        });

        // Implement the QueryFunction trait for queries which need it.
        if query.storage.needs_query_function() {
            let span = query.fn_name.span();
            let key_names: &Vec<_> = &(0..query.keys.len())
                .map(|i| Ident::new(&format!("key{}", i), Span::call_site()))
                .collect();
            let key_pattern = if query.keys.len() == 1 {
                quote! { #(#key_names),* }
            } else {
                quote! { (#(#key_names),*) }
            };
            let invoke = query.invoke_tt();

            let recover = if let Some(cycle_recovery_fn) = &query.cycle {
                quote! {
                    fn recover(db: &Self::DynDb, cycle: &[salsa::DatabaseKeyIndex], #key_pattern: &<Self as salsa::Query>::Key)
                        -> Option<<Self as salsa::Query>::Value> {
                        Some(#cycle_recovery_fn(
                                db,
                                &cycle.iter().map(|k| format!("{:?}", k.debug(db))).collect::<Vec<String>>(),
                                #(#key_names),*
                        ))
                    }
                }
            } else {
                quote! {}
            };

            output.extend(quote_spanned! {span=>
                // ANCHOR:QueryFunction_impl
                impl salsa::plumbing::QueryFunction for #qt
                {
                    fn execute(db: &Self::DynDb, #key_pattern: <Self as salsa::Query>::Key)
                        -> <Self as salsa::Query>::Value {
                        #invoke(db, #(#key_names),*)
                    }

                    #recover
                }
                // ANCHOR_END:QueryFunction_impl
            });
        }
    }

    let mut fmt_ops = proc_macro2::TokenStream::new();
    for (Query { fn_name, .. }, query_index) in non_transparent_queries().zip(0_u16..) {
        fmt_ops.extend(quote! {
            #query_index => {
                salsa::plumbing::QueryStorageOps::fmt_index(
                    &*self.#fn_name, db, input, fmt,
                )
            }
        });
    }

    let mut maybe_changed_ops = proc_macro2::TokenStream::new();
    for (Query { fn_name, .. }, query_index) in non_transparent_queries().zip(0_u16..) {
        maybe_changed_ops.extend(quote! {
            #query_index => {
                salsa::plumbing::QueryStorageOps::maybe_changed_since(
                    &*self.#fn_name, db, input, revision
                )
            }
        });
    }

    let mut for_each_ops = proc_macro2::TokenStream::new();
    for Query { fn_name, .. } in non_transparent_queries() {
        for_each_ops.extend(quote! {
            op(&*self.#fn_name);
        });
    }

    // Emit query group storage struct
    output.extend(quote! {
        #trait_vis struct #group_storage {
            #storage_fields
        }

        // ANCHOR:group_storage_new
        impl #group_storage {
            #trait_vis fn new(group_index: u16) -> Self {
                #group_storage {
                    #(
                        #queries_with_storage:
                        std::sync::Arc::new(salsa::plumbing::QueryStorageOps::new(group_index)),
                    )*
                }
            }
        }
        // ANCHOR_END:group_storage_new

        // ANCHOR:group_storage_methods
        impl #group_storage {
            #trait_vis fn fmt_index(
                &self,
                db: &#dyn_db,
                input: salsa::DatabaseKeyIndex,
                fmt: &mut std::fmt::Formatter<'_>,
            ) -> std::fmt::Result {
                match input.query_index() {
                    #fmt_ops
                    i => panic!("salsa: impossible query index {}", i),
                }
            }

            #trait_vis fn maybe_changed_since(
                &self,
                db: &#dyn_db,
                input: salsa::DatabaseKeyIndex,
                revision: salsa::Revision,
            ) -> bool {
                match input.query_index() {
                    #maybe_changed_ops
                    i => panic!("salsa: impossible query index {}", i),
                }
            }

            #trait_vis fn for_each_query(
                &self,
                _runtime: &salsa::Runtime,
                mut op: &mut dyn FnMut(&dyn salsa::plumbing::QueryStorageMassOps),
            ) {
                #for_each_ops
            }
        }
        // ANCHOR_END:group_storage_methods
    });

    if std::env::var("SALSA_DUMP").is_ok() {
        println!("~~~ query_group");
        println!("{}", output.to_string());
        println!("~~~ query_group");
    }

    output.into()
}

struct SalsaAttr {
    name: String,
    tts: TokenStream,
    span: Span,
}

impl std::fmt::Debug for SalsaAttr {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{:?}", self.name)
    }
}

impl TryFrom<syn::Attribute> for SalsaAttr {
    type Error = syn::Attribute;

    fn try_from(attr: syn::Attribute) -> Result<SalsaAttr, syn::Attribute> {
        if is_not_salsa_attr_path(&attr.path) {
            return Err(attr);
        }

        let span = attr.span();
        let name = attr.path.segments[1].ident.to_string();
        let tts = attr.tokens.into();

        Ok(SalsaAttr { name, tts, span })
    }
}

fn is_not_salsa_attr_path(path: &syn::Path) -> bool {
    path.segments
        .first()
        .map(|s| s.ident != "salsa")
        .unwrap_or(true)
        || path.segments.len() != 2
}

fn filter_attrs(attrs: Vec<Attribute>) -> (Vec<Attribute>, Vec<SalsaAttr>) {
    let mut other = vec![];
    let mut salsa = vec![];
    // Leave non-salsa attributes untouched. These are
    // attributes that don't start with `salsa::` or don't have
    // exactly two segments in their path.
    // Keep the salsa attributes around.
    for attr in attrs {
        match SalsaAttr::try_from(attr) {
            Ok(it) => salsa.push(it),
            Err(it) => other.push(it),
        }
    }
    (other, salsa)
}

#[derive(Debug)]
struct Query {
    fn_name: Ident,
    query_name: String,
    attrs: Vec<syn::Attribute>,
    query_type: Ident,
    storage: QueryStorage,
    keys: Vec<syn::Type>,
    value: syn::Type,
    invoke: Option<syn::Path>,
    cycle: Option<syn::Path>,
}

impl Query {
    fn invoke_tt(&self) -> proc_macro2::TokenStream {
        match &self.invoke {
            Some(i) => i.into_token_stream(),
            None => self.fn_name.clone().into_token_stream(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryStorage {
    Memoized,
    Dependencies,
    Input,
    Interned,
    InternedLookup { intern_query_type: Ident },
    Transparent,
}

impl QueryStorage {
    /// Do we need a `QueryFunction` impl for this type of query?
    fn needs_query_function(&self) -> bool {
        match self {
            QueryStorage::Input
            | QueryStorage::Interned
            | QueryStorage::InternedLookup { .. }
            | QueryStorage::Transparent => false,
            QueryStorage::Memoized | QueryStorage::Dependencies => true,
        }
    }

    /// Does this type of query support `&mut` operations?
    fn supports_mut(&self) -> bool {
        match self {
            QueryStorage::Input => true,
            QueryStorage::Interned
            | QueryStorage::InternedLookup { .. }
            | QueryStorage::Transparent
            | QueryStorage::Memoized
            | QueryStorage::Dependencies => false,
        }
    }
}
