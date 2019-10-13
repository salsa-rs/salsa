use heck::SnakeCase;
use proc_macro::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, ItemStruct, Path, Token};

type PunctuatedQueryGroups = Punctuated<QueryGroup, Token![,]>;

pub(crate) fn database(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(args as QueryGroupList);
    let input = syn::parse_macro_input!(input as ItemStruct);

    let query_groups = &args.query_groups;
    let database_name = &input.ident;
    let visibility = &input.vis;

    let mut output = proc_macro2::TokenStream::new();
    output.extend(quote! { #input });

    let query_group_names_snake: Vec<_> = query_groups
        .iter()
        .map(|query_group| {
            let group_name = query_group.name();
            Ident::new(&group_name.to_string().to_snake_case(), group_name.span())
        })
        .collect();

    let query_group_storage_names: Vec<_> = query_groups
        .iter()
        .map(|QueryGroup { group_path }| {
            quote! {
                <#group_path as salsa::plumbing::QueryGroup<#database_name>>::GroupStorage
            }
        })
        .collect();

    let query_group_key_names: Vec<_> = query_groups
        .iter()
        .map(|QueryGroup { group_path }| {
            quote! {
                <#group_path as salsa::plumbing::QueryGroup<#database_name>>::GroupKey
            }
        })
        .collect();

    // For each query group `foo::MyGroup` create a link to its
    // `foo::MyGroupGroupStorage`
    let mut storage_fields = proc_macro2::TokenStream::new();
    let mut has_group_impls = proc_macro2::TokenStream::new();
    for (((query_group, group_name_snake), group_storage), group_key) in query_groups
        .iter()
        .zip(&query_group_names_snake)
        .zip(&query_group_storage_names)
        .zip(&query_group_key_names)
    {
        let group_path = &query_group.group_path;
        let group_name = query_group.name();

        // rewrite the last identifier (`MyGroup`, above) to
        // (e.g.) `MyGroupGroupStorage`.
        storage_fields.extend(quote! {
            #group_name_snake: #group_storage,
        });
        has_group_impls.extend(quote! {
            impl salsa::plumbing::HasQueryGroup<#group_path> for #database_name {
                fn group_storage(db: &Self) -> &#group_storage {
                    let runtime = salsa::Database::salsa_runtime(db);
                    &runtime.storage().#group_name_snake
                }

                fn database_key(group_key: #group_key) -> __SalsaDatabaseKey {
                    __SalsaDatabaseKey {
                        kind: __SalsaDatabaseKeyKind::#group_name(group_key),
                    }
                }
            }
        });
    }

    // create group storage wrapper struct
    output.extend(quote! {
        #[derive(Default)]
        #[doc(hidden)]
        #visibility struct __SalsaDatabaseStorage {
            #storage_fields
        }
    });

    // create query database_key wrapper struct
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[doc(hidden)]
        #visibility struct __SalsaDatabaseKey {
            kind: __SalsaDatabaseKeyKind
        }
    });

    // For each query `fn foo() for FooType` create
    //
    // ```
    // foo(<FooType as salsa::Query<#database_name>>::Key),
    // ```
    let mut variants = proc_macro2::TokenStream::new();
    for (query_group, group_key) in query_groups.iter().zip(&query_group_key_names) {
        let group_name = query_group.name();
        variants.extend(quote!(
            #group_name(#group_key),
        ));
    }
    output.extend(quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        enum __SalsaDatabaseKeyKind {
            #variants
        }
    });

    // Create a tuple (D1, D2, ...) where Di is the data for a given query group.
    let mut database_data = vec![];
    for QueryGroup { group_path } in query_groups {
        database_data.push(quote! {
            <#group_path as salsa::plumbing::QueryGroup<#database_name>>::GroupData
        });
    }

    //
    output.extend(quote! {
        impl salsa::plumbing::DatabaseStorageTypes for #database_name {
            type DatabaseKey = __SalsaDatabaseKey;
            type DatabaseStorage = __SalsaDatabaseStorage;
            type DatabaseData = (#(#database_data),*);
        }
    });

    //
    let mut for_each_ops = proc_macro2::TokenStream::new();
    for (QueryGroup { group_path }, group_storage) in
        query_groups.iter().zip(&query_group_storage_names)
    {
        for_each_ops.extend(quote! {
            let storage: &#group_storage =
                <Self as salsa::plumbing::HasQueryGroup<#group_path>>::group_storage(self);
            storage.for_each_query(self, &mut op);
        });
    }
    output.extend(quote! {
        impl salsa::plumbing::DatabaseOps for #database_name {
            fn for_each_query(
                &self,
                mut op: impl FnMut(&dyn salsa::plumbing::QueryStorageMassOps<Self>),
            ) {
                #for_each_ops
            }
        }
    });

    output.extend(quote! {
        impl salsa::plumbing::DatabaseKey<#database_name> for __SalsaDatabaseKey {
        }
    });

    output.extend(has_group_impls);

    if std::env::var("SALSA_DUMP").is_ok() {
        println!("~~~ database_storage");
        println!("{}", output.to_string());
        println!("~~~ database_storage");
    }

    output.into()
}

#[derive(Clone, Debug)]
struct QueryGroupList {
    query_groups: PunctuatedQueryGroups,
}

impl Parse for QueryGroupList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let query_groups: PunctuatedQueryGroups = input.parse_terminated(QueryGroup::parse)?;
        Ok(QueryGroupList { query_groups })
    }
}

#[derive(Clone, Debug)]
struct QueryGroup {
    group_path: Path,
}

impl QueryGroup {
    /// The name of the query group trait.
    fn name(&self) -> Ident {
        self.group_path.segments.last().unwrap().ident.clone()
    }
}

impl Parse for QueryGroup {
    /// ```ignore
    ///         impl HelloWorldDatabase;
    /// ```
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let group_path: Path = input.parse()?;
        Ok(QueryGroup { group_path })
    }
}

struct Nothing;

impl Parse for Nothing {
    fn parse(_input: ParseStream) -> syn::Result<Self> {
        Ok(Nothing)
    }
}
