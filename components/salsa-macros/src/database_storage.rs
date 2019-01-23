use crate::parenthesized::Parenthesized;
use heck::SnakeCase;
use proc_macro::TokenStream;
use proc_macro2::Span;
use syn::parse::{Parse, ParseStream, Peek};
use syn::{Attribute, Ident, Path, Token, Visibility};

/// Implementation for `salsa::database_storage!` macro.
///
/// Current syntax:
///
/// ```ignore
///  salsa::database_storage! {
///     struct DatabaseStorage for DatabaseStruct {
///         impl HelloWorldDatabase {
///             fn input_string() for InputString;
///             fn length() for LengthQuery;
///         }
///     }
/// }
/// ```
///
/// impl Database {
pub(crate) fn database_storage(input: TokenStream) -> TokenStream {
    let DatabaseStorage {
        database_name,
        query_groups,
        attributes,
        visibility,
    } = syn::parse_macro_input!(input as DatabaseStorage);

    let mut output = proc_macro2::TokenStream::new();

    let query_group_names_camel: Vec<_> = query_groups
        .iter()
        .map(|query_group| {
            let group_storage = query_group.query_group.clone();
            group_storage.segments.last().unwrap().value().ident.clone()
        })
        .collect();

    let query_group_names_snake: Vec<_> = query_group_names_camel
        .iter()
        .map(|query_group_name_camel| {
            Ident::new(
                &query_group_name_camel.to_string().to_snake_case(),
                query_group_name_camel.span(),
            )
        })
        .collect();

    let each_query = || {
        query_groups
            .iter()
            .zip(&query_group_names_snake)
            .flat_map(|(query_group, name_snake)| {
                query_group.queries.iter().map(move |q| (name_snake, q))
            })
    };

    // For each query group `foo::MyGroup` create a link to its
    // `foo::MyGroupGroupStorage`
    let mut storage_fields = proc_macro2::TokenStream::new();
    for (query_group, query_group_name_snake) in query_groups.iter().zip(&query_group_names_snake) {
        // rewrite the last identifier (`MyGroup`, above) to
        // (e.g.) `MyGroupGroupStorage`.
        let mut group_storage = query_group.query_group.clone();
        let last_ident = &group_storage.segments.last().unwrap().value().ident;
        let storage_ident = Ident::new(
            &format!("{}GroupStorage", last_ident.to_string()),
            Span::call_site(),
        );
        group_storage.segments.last_mut().unwrap().value_mut().ident = storage_ident;
        storage_fields.extend(quote! { #query_group_name_snake: #group_storage<#database_name>, });
    }

    let mut attrs = proc_macro2::TokenStream::new();
    for attr in attributes {
        attrs.extend(quote! { #attr });
    }

    // create group storage wrapper struct
    output.extend(quote! {
        #[derive(Default)]
        #[doc(hidden)]
        #visibility struct __SalsaDatabaseStorage {
            #storage_fields
        }
    });

    // create query descriptor wrapper struct
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[doc(hidden)]
        #visibility struct __SalsaQueryDescriptor {
            kind: __SalsaQueryDescriptorKind
        }
    });

    // For each query `fn foo() for FooType` create
    //
    // ```
    // foo(<FooType as ::salsa::Query<#database_name>>::Key),
    // ```
    let mut variants = proc_macro2::TokenStream::new();
    for (
        _,
        Query {
            query_name,
            query_type,
        },
    ) in each_query()
    {
        variants.extend(quote!(
            #query_name(<#query_type as ::salsa::Query<#database_name>>::Key),
        ));
    }
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        enum __SalsaQueryDescriptorKind {
            #variants
        }
    });

    //
    output.extend(quote! {
        impl ::salsa::plumbing::DatabaseStorageTypes for #database_name {
            type QueryDescriptor = __SalsaQueryDescriptor;
            type DatabaseStorage = __SalsaDatabaseStorage;
        }
    });

    //
    let mut for_each_ops = proc_macro2::TokenStream::new();
    for (group_index, Query { query_name, .. }) in each_query() {
        for_each_ops.extend(quote! {
            op(&::salsa::Database::salsa_runtime(self).storage().#group_index.#query_name);
        });
    }
    output.extend(quote! {
        impl ::salsa::plumbing::DatabaseOps for #database_name {
            fn for_each_query(
                &self,
                mut op: impl FnMut(&dyn ::salsa::plumbing::QueryStorageMassOps<Self>),
            ) {
                #for_each_ops
            }
        }
    });

    let mut for_each_query_desc = proc_macro2::TokenStream::new();
    for (
        group_index,
        Query {
            query_name,
            query_type,
        },
    ) in each_query()
    {
        for_each_query_desc.extend(quote! {
            __SalsaQueryDescriptorKind::#query_name(key) => {
                let runtime = ::salsa::Database::salsa_runtime(db);
                let storage = &runtime.storage().#group_index.#query_name;
                <_ as ::salsa::plumbing::QueryStorageOps<#database_name, #query_type>>::maybe_changed_since(
                    storage,
                    db,
                    revision,
                    key,
                    self,
                )
            }
        });
    }

    output.extend(quote! {
        impl ::salsa::plumbing::QueryDescriptor<#database_name> for __SalsaQueryDescriptor {
            fn maybe_changed_since(
                &self,
                db: &#database_name,
                revision: ::salsa::plumbing::Revision,
            ) -> bool {
                match &self.kind {
                    #for_each_query_desc
                }
            }
        }
    });

    let mut for_each_query_table = proc_macro2::TokenStream::new();
    for (
        group_index,
        Query {
            query_name,
            query_type,
        },
    ) in each_query()
    {
        for_each_query_table.extend(quote! {
            impl ::salsa::plumbing::GetQueryTable<#query_type> for #database_name {
                fn get_query_table(
                    db: &Self,
                ) -> ::salsa::QueryTable<'_, Self, #query_type> {
                    ::salsa::QueryTable::new(
                        db,
                        &::salsa::Database::salsa_runtime(db)
                            .storage()
                            .#group_index
                            .#query_name,
                    )
                }

                fn get_query_table_mut(
                    db: &mut Self,
                ) -> ::salsa::QueryTableMut<'_, Self, #query_type> {
                    let db = &*db;
                    ::salsa::QueryTableMut::new(
                        db,
                        &::salsa::Database::salsa_runtime(db)
                            .storage()
                            .#group_index
                            .#query_name,
                    )
                }

                fn descriptor(
                    db: &Self,
                    key: <#query_type as ::salsa::Query<Self>>::Key,
                ) -> <Self as ::salsa::plumbing::DatabaseStorageTypes>::QueryDescriptor {
                    __SalsaQueryDescriptor {
                        kind: __SalsaQueryDescriptorKind::#query_name(key),
                    }
                }
            }
        });
    }

    output.extend(for_each_query_table);

    output.into()
}

struct DatabaseStorage {
    database_name: Path,
    query_groups: Vec<QueryGroup>,
    attributes: Vec<Attribute>,
    visibility: Visibility,
}

struct QueryGroup {
    query_group: Path,
    queries: Vec<Query>,
}

struct Query {
    query_name: Ident,
    query_type: Path,
}

impl Parse for DatabaseStorage {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attributes = input.call(Attribute::parse_outer)?;
        let visibility = input.parse()?;
        let _struct_token: Token![struct ] = input.parse()?;
        let _storage_struct_name: Ident = input.parse()?;
        let _for_token: Token![for ] = input.parse()?;
        let database_name: Path = input.parse()?;
        let content;
        syn::braced!(content in input);
        let query_groups: Vec<QueryGroup> = parse_while(Token![impl ], &content)?;
        Ok(DatabaseStorage {
            attributes,
            visibility,
            database_name,
            query_groups,
        })
    }
}

impl Parse for QueryGroup {
    /// ```ignore
    ///         impl HelloWorldDatabase {
    ///             fn input_string() for InputString;
    ///             fn length() for LengthQuery;
    ///         }
    /// ```
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _fn_token: Token![impl ] = input.parse()?;
        let query_group: Path = input.parse()?;
        let content;
        syn::braced!(content in input);
        let queries: Vec<Query> = parse_while(Token![fn ], &content)?;
        Ok(QueryGroup {
            query_group,
            queries,
        })
    }
}

impl Parse for Query {
    /// ```ignore
    ///             fn input_string() for InputString;
    /// ```
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _fn_token: Token![fn ] = input.parse()?;
        let query_name: Ident = input.parse()?;
        let _unit: Parenthesized<Nothing> = input.parse()?;
        let _for_token: Token![for ] = input.parse()?;
        let query_type: Path = input.parse()?;
        let _for_token: Token![;] = input.parse()?;
        Ok(Query {
            query_name,
            query_type,
        })
    }
}

struct Nothing;

impl Parse for Nothing {
    fn parse(_input: ParseStream) -> syn::Result<Self> {
        Ok(Nothing)
    }
}

fn parse_while<P: Peek + Copy, B: Parse>(peek: P, input: ParseStream) -> syn::Result<Vec<B>> {
    let mut result = vec![];
    while input.peek(peek) {
        let body: B = input.parse()?;
        result.push(body);
    }
    Ok(result)
}
