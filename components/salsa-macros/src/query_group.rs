use std::convert::TryFrom;

use crate::parenthesized::Parenthesized;
use heck::CamelCase;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, parse_quote, Attribute, FnArg, Ident, ItemTrait, Path, ReturnType, Token,
    TraitBound, TraitBoundModifier, TraitItem, Type, TypeParamBound,
};

/// Implementation for `[salsa::query_group]` decorator.
pub(crate) fn query_group(args: TokenStream, input: TokenStream) -> TokenStream {
    let group_struct = parse_macro_input!(args as Ident);
    let input: ItemTrait = parse_macro_input!(input as ItemTrait);
    // println!("args: {:#?}", args);
    // println!("input: {:#?}", input);

    let (trait_attrs, salsa_attrs) = filter_attrs(input.attrs);
    let mut requires: Punctuated<Path, Token![+]> = Punctuated::new();
    for SalsaAttr { name, tts } in salsa_attrs {
        match name.as_str() {
            "requires" => {
                requires.push(parse_macro_input!(tts as Parenthesized<syn::Path>).0);
            }
            _ => panic!("unknown salsa attribute `{}`", name),
        }
    }

    let trait_vis = input.vis;
    let trait_name = input.ident;
    let _generics = input.generics.clone();

    // Decompose the trait into the corresponding queries.
    let mut queries = vec![];
    for item in input.items {
        match item {
            TraitItem::Method(method) => {
                let mut storage = QueryStorage::Memoized;
                let mut cycle = None;
                let mut invoke = None;
                let mut query_type = Ident::new(
                    &format!("{}Query", method.sig.ident.to_string().to_camel_case()),
                    Span::call_site(),
                );
                let mut num_storages = 0;

                // Extract attributes.
                let (attrs, salsa_attrs) = filter_attrs(method.attrs);
                for SalsaAttr { name, tts } in salsa_attrs {
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
                let mut iter = method.sig.inputs.iter();
                match iter.next() {
                    Some(FnArg::Receiver(sr)) if sr.mutability.is_some() => (),
                    _ => panic!(
                        "first argument of query `{}` must be `&mut self`",
                        method.sig.ident
                    ),
                }
                let mut keys: Vec<Type> = vec![];
                for arg in iter {
                    match *arg {
                        FnArg::Typed(ref arg) => {
                            keys.push((*arg.ty).clone());
                        }
                        ref a => panic!("unsupported argument `{:?}` of `{}`", a, method.sig.ident),
                    }
                }

                // Extract value.
                let value = match method.sig.output {
                    ReturnType::Type(_, ref ty) => ty.as_ref().clone(),
                    ref r => panic!(
                        "unsupported return type `{:?}` of `{}`",
                        r, method.sig.ident
                    ),
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
                        is_async: method.sig.asyncness.is_some(),
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
                    is_async: method.sig.asyncness.is_some(),
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

    let group_key = Ident::new(
        &format!("{}GroupKey__", trait_name.to_string()),
        Span::call_site(),
    );

    let group_storage = Ident::new(
        &format!("{}GroupStorage__", trait_name.to_string()),
        Span::call_site(),
    );

    let mut query_fn_declarations = proc_macro2::TokenStream::new();
    let mut query_fn_definitions = proc_macro2::TokenStream::new();
    let mut query_descriptor_variants = proc_macro2::TokenStream::new();
    let mut group_data_elements = vec![];
    let mut storage_fields = proc_macro2::TokenStream::new();
    let mut storage_defaults = proc_macro2::TokenStream::new();
    for query in &queries {
        let key_names: &Vec<_> = &(0..query.keys.len())
            .map(|i| Ident::new(&format!("key{}", i), Span::call_site()))
            .collect();
        let keys = &query.keys;
        let value = &query.value;
        let fn_name = &query.fn_name;
        let qt = &query.query_type;
        let attrs = &query.attrs;

        if query.is_async {
            query_fn_declarations.extend(quote! {
                #(#attrs)*
                fn #fn_name<'s>(&'s mut self, #(#key_names: #keys),*) -> std::pin::Pin<Box<dyn std::future::Future<Output = #value> + Send + 's>>;
            });
        } else {
            query_fn_declarations.extend(quote! {
                #(#attrs)*
                fn #fn_name(&mut self, #(#key_names: #keys),*) -> #value;
            });
        }

        // Special case: transparent queries don't create actual storage,
        // just inline the definition
        if let QueryStorage::Transparent = query.storage {
            let invoke = query.invoke_tt();
            if query.is_async {
                query_fn_definitions.extend(quote! {
                    fn #fn_name<'s>(&'s mut self, #(#key_names: #keys),*) -> std::pin::Pin<Box<dyn std::future::Future<Output = #value> + Send + 's>> {
                        Box::pin(#invoke(self, #(#key_names),*))
                    }
                });
            } else {
                query_fn_definitions.extend(quote! {
                    fn #fn_name(&mut self, #(#key_names: #keys),*) -> #value {
                        #invoke(self, #(#key_names),*)
                    }
                });
            }
            continue;
        }

        if query.is_async {
            query_fn_definitions.extend(quote! {
                fn #fn_name<'s>(&'s mut self, #(#key_names: #keys),*) -> std::pin::Pin<Box<dyn std::future::Future<Output = #value> + Send + 's>> {
                    Box::pin(async move {
                        <Self as salsa::plumbing::GetQueryTable<#qt>>::get_query_table_mut(self).get_async((#(#key_names),*)).await
                    })
                }
            });
        } else {
            query_fn_definitions.extend(quote! {
                fn #fn_name(&mut self, #(#key_names: #keys),*) -> #value {
                    <Self as salsa::plumbing::GetQueryTable<#qt>>::get_query_table_mut(self).get((#(#key_names),*))
                }
            });
        }

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
                    <Self as salsa::plumbing::GetQueryTable<#qt>>::get_query_table_mut(self).set((#(#key_names),*), value__)
                }

                fn #set_with_durability_fn_name(&mut self, #(#key_names: #keys,)* value__: #value, durability__: salsa::Durability) {
                    <Self as salsa::plumbing::GetQueryTable<#qt>>::get_query_table_mut(self).set_with_durability((#(#key_names),*), value__, durability__)
                }
            });
        }

        // A variant for the group descriptor below
        query_descriptor_variants.extend(quote! {
            #fn_name((#(#keys),*)),
        });

        // Entry for the query group data tuple
        group_data_elements.push(quote! {
            (#(#keys,)* #value)
        });

        // A field for the storage struct
        //
        // FIXME(#120): the pub should not be necessary once we complete the transition
        storage_fields.extend(quote! {
            pub #fn_name: std::sync::Arc<<#qt as salsa::Query<DB__>>::Storage>,
        });
        storage_defaults.extend(quote! { #fn_name: Default::default(), });
    }

    // Emit the trait itself.
    let mut output = {
        let bounds = &input.supertraits;
        quote! {
            #(#trait_attrs)*
            #trait_vis trait #trait_name : #bounds {
                #query_fn_declarations
            }
        }
    };

    // Emit the query group struct and impl of `QueryGroup`.
    output.extend(quote! {
        /// Representative struct for the query group.
        #trait_vis struct #group_struct { }

        impl<DB__> salsa::plumbing::QueryGroup<DB__> for #group_struct
        where
            DB__: #trait_name + #requires,
            DB__: salsa::plumbing::HasQueryGroup<#group_struct>,
            DB__: salsa::Database,
        {
            type GroupStorage = #group_storage<DB__>;
            type GroupKey = #group_key;
            type GroupData = (#(#group_data_elements),*);
        }
    });

    // Emit an impl of the trait
    output.extend({
        let mut bounds = input.supertraits.clone();
        for path in requires.clone() {
            bounds.push(TypeParamBound::Trait(TraitBound {
                paren_token: None,
                modifier: TraitBoundModifier::None,
                lifetimes: None,
                path,
            }));
        }
        quote! {
            impl<T> #trait_name for T
            where
                T: #bounds,
                T: salsa::plumbing::HasQueryGroup<#group_struct>
            {
                #query_fn_definitions
            }
        }
    });

    // Emit the query types.
    for query in &queries {
        let fn_name = &query.fn_name;
        let qt = &query.query_type;

        let db = quote! {DB};

        let storage = match &query.storage {
            QueryStorage::Memoized => quote!(salsa::plumbing::MemoizedStorage<#db, Self>),
            QueryStorage::Dependencies => quote!(salsa::plumbing::DependencyStorage<#db, Self>),
            QueryStorage::Input => quote!(salsa::plumbing::InputStorage<#db, Self>),
            QueryStorage::Interned => quote!(salsa::plumbing::InternedStorage<#db, Self>),
            QueryStorage::InternedLookup { intern_query_type } => {
                quote!(salsa::plumbing::LookupInternedStorage<#db, Self, #intern_query_type>)
            }
            QueryStorage::Transparent => continue,
        };
        let keys = &query.keys;
        let value = &query.value;

        // Emit the query struct and implement the Query trait on it.
        output.extend(quote! {
            #[derive(Default, Debug)]
            #trait_vis struct #qt;

            // Unsafe proof obligation: that our key/value are a part
            // of the `GroupData`.
            unsafe impl<#db> salsa::Query<#db> for #qt
            where
                DB: #trait_name + #requires,
                DB: salsa::plumbing::HasQueryGroup<#group_struct>,
                DB: salsa::Database,
            {
                type Key = (#(#keys),*);
                type Value = #value;
                type Storage = #storage;
                type Group = #group_struct;
                type GroupStorage = #group_storage<#db>;
                type GroupKey = #group_key;

                fn query_storage(
                    group_storage: &Self::GroupStorage,
                ) -> &std::sync::Arc<Self::Storage> {
                    &group_storage.#fn_name
                }

                fn group_key(key: Self::Key) -> Self::GroupKey {
                    #group_key::#fn_name(key)
                }
            }
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
                    fn recover(db: &mut DB, cycle: &[DB::DatabaseKey], #key_pattern: &<Self as salsa::Query<DB>>::Key)
                        -> Option<<Self as salsa::Query<DB>>::Value> {
                        Some(#cycle_recovery_fn(
                                db,
                                &cycle.iter().map(|k| format!("{:?}", k)).collect::<Vec<String>>(),
                                #(#key_names),*
                        ))
                    }
                }
            } else {
                quote! {}
            };

            let future = if query.is_async {
                quote_spanned! {span=>
                    #invoke(db, #(#key_names),*)
                }
            } else {
                quote_spanned! {span=>
                    salsa::futures::future::ready(#invoke(db, #(#key_names),*))
                }
            };

            output.extend(quote_spanned! {span=>
                impl<DB> salsa::plumbing::QueryFunction<DB> for #qt
                where
                    DB: #trait_name + #requires,
                    DB: salsa::plumbing::HasQueryGroup<#group_struct>,
                    DB: salsa::Database,
                {
                    fn execute<'a>(db: &'a mut DB, #key_pattern: <Self as salsa::Query<DB>>::Key)
                        -> salsa::BoxFutureLocal<'a, <Self as salsa::Query<DB>>::Value> {
                        Box::pin(#future)
                    }

                    #recover
                }
            });
        }
    }

    // Emit query group descriptor
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[allow(non_camel_case_types)]
        #trait_vis enum #group_key {
            #query_descriptor_variants
        }
    });

    let mut for_each_ops = proc_macro2::TokenStream::new();
    for Query { fn_name, .. } in queries
        .iter()
        .filter(|q| q.storage != QueryStorage::Transparent)
    {
        for_each_ops.extend(quote! {
            op(&*self.#fn_name);
        });
    }

    // Emit query group storage struct
    // It would derive Default, but then all database structs would have to implement Default
    // as the derived version includes an unused `+ Default` constraint.
    output.extend(quote! {
        #trait_vis struct #group_storage<DB__>
        where
            DB__: #trait_name + #requires,
            DB__: salsa::plumbing::HasQueryGroup<#group_struct>,
            DB__: salsa::Database,
        {
            #storage_fields
        }

        impl<DB__> Default for #group_storage<DB__>
        where
            DB__: #trait_name + #requires,
            DB__: salsa::plumbing::HasQueryGroup<#group_struct>,
            DB__: salsa::Database,
        {
            #[inline]
            fn default() -> Self {
                #group_storage {
                    #storage_defaults
                }
            }
        }

        impl<DB__> #group_storage<DB__>
        where
            DB__: #trait_name + #requires,
            DB__: salsa::plumbing::HasQueryGroup<#group_struct>,
        {
            #trait_vis fn for_each_query(
                &self,
                db: &DB__,
                mut op: &mut dyn FnMut(&dyn salsa::plumbing::QueryStorageMassOps<DB__>),
            ) {
                #for_each_ops
            }
        }
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
}

impl TryFrom<syn::Attribute> for SalsaAttr {
    type Error = syn::Attribute;
    fn try_from(attr: syn::Attribute) -> Result<SalsaAttr, syn::Attribute> {
        if is_not_salsa_attr_path(&attr.path) {
            return Err(attr);
        }

        let name = attr.path.segments[1].ident.to_string();
        let tts = attr.tokens.into();
        Ok(SalsaAttr { name, tts })
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
    attrs: Vec<syn::Attribute>,
    is_async: bool,
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
    fn needs_query_function(&self) -> bool {
        match self {
            QueryStorage::Input
            | QueryStorage::Interned
            | QueryStorage::InternedLookup { .. }
            | QueryStorage::Transparent => false,
            QueryStorage::Memoized | QueryStorage::Dependencies => true,
        }
    }
}
