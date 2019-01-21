use crate::parenthesized::Parenthesized;
use proc_macro::TokenStream;
use syn::parse::{Parse, ParseStream, Peek};
use syn::{Ident, Path, Token};

/// Implementation for `salsa::database_storage!` macro.
///
/// Current syntax:
///
/// ```
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
        storage_struct_name,
        database_name,
        query_groups,
    } = syn::parse_macro_input!(input as DatabaseStorage);

    let each_query = || {
        query_groups
            .iter()
            .flat_map(|query_group| &query_group.queries)
    };

    // For each query `fn foo() for FooType` create
    //
    // ```
    // foo: <FooType as ::salsa::Query<#database_name>>::Storage,
    // ```
    let mut fields = proc_macro2::TokenStream::new();
    for Query {
        query_name,
        query_type,
    } in each_query()
    {
        fields.extend(quote! {
            #query_name: <#query_type as ::salsa::Query<#database_name>>::Storage,
        });
    }

    // Create the storage struct defintion
    let mut output = quote! {
        #[derive(Default)]
        // XXX attributes
        // XXX visibility
        struct #storage_struct_name {
            #fields
        }
    };

    // create query descriptor wrapper struct
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        // XXX visibility
        struct __SalsaQueryDescriptor {
            kind: __SalsaQueryDescriptorKind
        }
    });

    // For each query `fn foo() for FooType` create
    //
    // ```
    // foo(<FooType as ::salsa::Query<#database_name>>::Key),
    // ```
    let mut variants = proc_macro2::TokenStream::new();
    for Query {
        query_name,
        query_type,
    } in each_query()
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
            type DatabaseStorage = #storage_struct_name;
        }
    });

    //
    let mut for_each_ops = proc_macro2::TokenStream::new();
    for Query { query_name, .. } in each_query() {
        for_each_ops.extend(quote! {
            op(&::salsa::Database::salsa_runtime(self).storage().#query_name);
        });
    }
    output.extend(quote! {
        impl ::salsa::plumbing::DatabaseOps for #database_name {
            fn for_each_query(
                &self,
                mut op: impl FnMut(&dyn $crate::plumbing::QueryStorageMassOps<Self>),
            ) {
                #for_each_ops
            }
        }
    });

    let mut for_each_query_desc = proc_macro2::TokenStream::new();
    for Query {
        query_name,
        query_type,
    } in each_query()
    {
        for_each_query_desc.extend(quote! {
            __SalsaQueryDescriptorKind::#query_name(key) => {
                let runtime = $crate::Database::salsa_runtime(db);
                let storage = &runtime.storage().#query_name;
                <_ as $crate::plumbing::QueryStorageOps<#database_name, #query_type>>::maybe_changed_since(
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
                revision: $crate::plumbing::Revision,
            ) -> bool {
                match &self.kind {
                    #for_each_query_desc
                }
            }
        }
    });

    let mut for_each_query_table = proc_macro2::TokenStream::new();
    for Query {
        query_name,
        query_type,
    } in each_query()
    {
        for_each_query_table.extend(quote! {
            impl $crate::plumbing::GetQueryTable<#query_type> for #database_name {
                fn get_query_table(
                    db: &Self,
                ) -> $crate::QueryTable<'_, Self, #query_type> {
                    $crate::QueryTable::new(
                        db,
                        &$crate::Database::salsa_runtime(db)
                            .storage()
                            .#query_name,
                    )
                }

                fn get_query_table_mut(
                    db: &mut Self,
                ) -> $crate::QueryTableMut<'_, Self, #query_type> {
                    let db = &*db;
                    $crate::QueryTableMut::new(
                        db,
                        &$crate::Database::salsa_runtime(db)
                            .storage()
                            .#query_name,
                    )
                }

                fn descriptor(
                    db: &Self,
                    key: <#query_type as $crate::Query<Self>>::Key,
                ) -> <Self as $crate::plumbing::DatabaseStorageTypes>::QueryDescriptor {
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
    storage_struct_name: Ident,
    database_name: Path,
    query_groups: Vec<QueryGroup>,
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
        let _struct_token: Token![struct ] = input.parse()?;
        let storage_struct_name: Ident = input.parse()?;
        let _for_token: Token![for ] = input.parse()?;
        let database_name: Path = input.parse()?;
        let content;
        syn::braced!(content in input);
        let query_groups: Vec<QueryGroup> = parse_while(Token![impl ], &content)?;
        Ok(DatabaseStorage {
            storage_struct_name,
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
