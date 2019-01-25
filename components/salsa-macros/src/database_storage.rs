use heck::SnakeCase;
use proc_macro::TokenStream;
use proc_macro2::Span;
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

    // For each query group `foo::MyGroup` create a link to its
    // `foo::MyGroupGroupStorage`
    let mut storage_fields = proc_macro2::TokenStream::new();
    let mut has_group_impls = proc_macro2::TokenStream::new();
    for (query_group, query_group_name_snake) in query_groups.iter().zip(&query_group_names_snake) {
        let group_name = query_group.name();
        let group_storage = query_group.group_storage();
        let group_key = query_group.group_key();

        // rewrite the last identifier (`MyGroup`, above) to
        // (e.g.) `MyGroupGroupStorage`.
        storage_fields.extend(quote! { #query_group_name_snake: #group_storage<#database_name>, });
        has_group_impls.extend(quote! {
            impl ::salsa::plumbing::HasQueryGroup<#group_storage<#database_name>, #group_key>
                for #database_name
            {
                fn group_storage(db: &Self) -> &#group_storage<#database_name> {
                    let runtime = ::salsa::Database::salsa_runtime(db);
                    &runtime.storage().#query_group_name_snake
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
    // foo(<FooType as ::salsa::Query<#database_name>>::Key),
    // ```
    let mut variants = proc_macro2::TokenStream::new();
    for query_group in query_groups {
        let group_name = query_group.name();
        let group_key = query_group.group_key();
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

    //
    output.extend(quote! {
        impl ::salsa::plumbing::DatabaseStorageTypes for #database_name {
            type DatabaseKey = __SalsaDatabaseKey;
            type DatabaseStorage = __SalsaDatabaseStorage;
        }
    });

    //
    let mut for_each_ops = proc_macro2::TokenStream::new();
    for query_group in query_groups {
        let group_storage = query_group.group_storage();
        for_each_ops.extend(quote! {
            let storage: &#group_storage<#database_name> =
                ::salsa::plumbing::HasQueryGroup::group_storage(self);
            storage.for_each_query(self, &mut op);
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
    for query_group in query_groups {
        let group_name = query_group.name();
        for_each_query_desc.extend(quote! {
            __SalsaDatabaseKeyKind::#group_name(database_key) => database_key.maybe_changed_since(
                db,
                self,
                revision,
            ),
        });
    }

    output.extend(quote! {
        impl ::salsa::plumbing::DatabaseKey<#database_name> for __SalsaDatabaseKey {
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
    query_group: Path,
}

impl QueryGroup {
    /// The name of the query group trait.
    fn name(&self) -> Ident {
        self.query_group
            .segments
            .last()
            .unwrap()
            .value()
            .ident
            .clone()
    }

    /// Construct the path to the group storage for a query group. For
    /// a query group at the path `foo::MyQuery`, this would be
    /// `foo::MyQueryGroupStorage`.
    fn group_storage(&self) -> Path {
        self.path_with_suffix("GroupStorage")
    }

    /// Construct the path to the group storage for a query group. For
    /// a query group at the path `foo::MyQuery`, this would be
    /// `foo::MyQueryGroupDatabaseKey`.
    fn group_key(&self) -> Path {
        self.path_with_suffix("GroupKey")
    }

    /// Construct a path leading to the query group, but with some
    /// suffix. So, for a query group at the path `foo::MyQuery`,
    /// this would be `foo::MyQueryXXX` where `XXX` is the provided
    /// suffix.
    fn path_with_suffix(&self, suffix: &str) -> Path {
        let mut group_storage = self.query_group.clone();
        let last_ident = &group_storage.segments.last().unwrap().value().ident;
        let storage_ident = Ident::new(
            &format!("{}{}", last_ident.to_string(), suffix),
            Span::call_site(),
        );
        group_storage.segments.last_mut().unwrap().value_mut().ident = storage_ident;
        group_storage
    }
}

impl Parse for QueryGroup {
    /// ```ignore
    ///         impl HelloWorldDatabase;
    /// ```
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let query_group: Path = input.parse()?;
        Ok(QueryGroup { query_group })
    }
}

struct Nothing;

impl Parse for Nothing {
    fn parse(_input: ParseStream) -> syn::Result<Self> {
        Ok(Nothing)
    }
}
