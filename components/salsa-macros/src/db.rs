use proc_macro2::Literal;
use syn::{spanned::Spanned, Token};

// Source:
//
// #[salsa::db(Jar0, Jar1, Jar2)]
// pub struct Database {
//    storage: salsa::Storage<Self>,
// }

pub(crate) fn db(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(args as Args);
    let input = syn::parse_macro_input!(input as syn::ItemStruct);
    let storage = match find_storage_field(&input) {
        Ok(v) => v,
        Err(err) => {
            let err = Literal::string(err);
            let error = quote_spanned!(input.ident.span() => compile_error!(#err));
            return quote! {
                #input
                #error
            }
            .into();
        }
    };

    let as_salsa_database_impl = as_salsa_database_impl(&input);
    let has_jars_impl = has_jars_impl(&args, &input, &storage);
    let has_jars_dyn_impl = has_jars_dyn_impl(&input, &storage);
    let per_jar_impls = per_jar_impls(&args, &input, &storage);

    quote! {
        #input
        #as_salsa_database_impl
        #has_jars_impl
        #has_jars_dyn_impl
        #(#per_jar_impls)*
    }
    .into()
}

pub struct Args {
    jar_paths: syn::punctuated::Punctuated<syn::Path, Token![,]>,
}

impl syn::parse::Parse for Args {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            jar_paths: syn::punctuated::Punctuated::parse_terminated(input)?,
        })
    }
}

fn find_storage_field(input: &syn::ItemStruct) -> Result<syn::Ident, &'static str> {
    let storage = "storage";
    for field in input.fields.iter() {
        if let Some(i) = &field.ident {
            if i == storage {
                return Ok(i.clone());
            }
        } else {
            return Err(
                "database struct must be a braced struct (`{}`) with a field named storage",
            );
        }
    }

    Err("database has no field named `storage`")
}

fn as_salsa_database_impl(input: &syn::ItemStruct) -> syn::ItemImpl {
    let db = &input.ident;
    parse_quote! {
        impl salsa::database::AsSalsaDatabase for #db {
            fn as_salsa_database(&self) -> &dyn salsa::Database {
                self
            }
        }
    }
}

fn has_jars_impl(args: &Args, input: &syn::ItemStruct, storage: &syn::Ident) -> syn::ItemImpl {
    let jar_paths: Vec<&syn::Path> = args.jar_paths.iter().collect();
    let jar_field_names: Vec<_> = args
        .jar_paths
        .iter()
        .zip(0..)
        .map(|(p, i)| syn::LitInt::new(&format!("{}", i), p.span()))
        .collect();
    let db = &input.ident;
    parse_quote! {
        // ANCHOR: HasJars
        impl salsa::storage::HasJars for #db {
            type Jars = (#(#jar_paths,)*);
            // ANCHOR_END: HasJars

            fn jars(&self) -> (&Self::Jars, &salsa::Runtime) {
                self.#storage.jars()
            }

            fn jars_mut(&mut self) -> (&mut Self::Jars, &mut salsa::Runtime) {
                self.#storage.jars_mut()
            }

            // ANCHOR: create_jars
            fn create_jars(routes: &mut salsa::routes::Routes<Self>) -> Box<Self::Jars> {
                unsafe {
                    salsa::plumbing::create_jars_inplace::<#db>(|jars| {
                        #(
                            unsafe {
                                let place = std::ptr::addr_of_mut!((*jars).#jar_field_names);
                                <#jar_paths as salsa::jar::Jar>::init_jar(place, routes);
                            }
                        )*
                    })
                }
            }
            // ANCHOR_END: create_jars
        }
    }
}

fn has_jars_dyn_impl(input: &syn::ItemStruct, storage: &syn::Ident) -> syn::ItemImpl {
    let db = &input.ident;
    parse_quote! {
        impl salsa::storage::HasJarsDyn for #db {
            fn runtime(&self) -> &salsa::Runtime {
                self.#storage.runtime()
            }

            fn runtime_mut(&mut self) ->&mut salsa::Runtime {
                self.#storage.runtime_mut()
            }

            fn maybe_changed_after(
                &self,
                input: salsa::key::DependencyIndex,
                revision: salsa::Revision,
            ) -> bool {
                let ingredient = self.#storage.ingredient(input.ingredient_index());
                ingredient.maybe_changed_after(self, input, revision)
            }

            fn cycle_recovery_strategy(
                &self,
                ingredient_index: salsa::IngredientIndex,
            ) -> salsa::cycle::CycleRecoveryStrategy {
                let ingredient = self.#storage.ingredient(ingredient_index);
                ingredient.cycle_recovery_strategy()
            }

            fn origin(
                &self,
                index: salsa::DatabaseKeyIndex,
            ) -> Option<salsa::runtime::local_state::QueryOrigin> {
                let ingredient = self.#storage.ingredient(index.ingredient_index());
                ingredient.origin(index.key_index())
            }

            fn mark_validated_output(&self, executor: salsa::DatabaseKeyIndex, output: salsa::key::DependencyIndex) {
                let ingredient = self.#storage.ingredient(output.ingredient_index());
                ingredient.mark_validated_output(self, executor, output.key_index());
            }

            fn remove_stale_output(&self, executor: salsa::DatabaseKeyIndex, stale_output: salsa::key::DependencyIndex) {
                let ingredient = self.#storage.ingredient(stale_output.ingredient_index());
                ingredient.remove_stale_output(self, executor, stale_output.key_index());
            }

            fn salsa_struct_deleted(&self, ingredient: salsa::IngredientIndex, id: salsa::Id) {
                let ingredient = self.#storage.ingredient(ingredient);
                ingredient.salsa_struct_deleted(self, id);
            }

            fn fmt_index(&self, index: salsa::key::DependencyIndex, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let ingredient = self.#storage.ingredient(index.ingredient_index());
                ingredient.fmt_index(index.key_index(), fmt)
            }
        }
    }
}

fn per_jar_impls(args: &Args, input: &syn::ItemStruct, storage: &syn::Ident) -> Vec<syn::ItemImpl> {
    let db = &input.ident;
    args.jar_paths
        .iter()
        .zip(0..)
        .flat_map(|(jar_path, jar_index)| {
            let jar_index = Literal::u32_unsuffixed(jar_index);
            vec![
                parse_quote! {
                    impl salsa::storage::DbWithJar<#jar_path> for #db {
                        fn as_jar_db<'db>(&'db self) -> &'db <#jar_path as salsa::jar::Jar<'db>>::DynDb
                        where
                            'db: 'db,
                        {
                            self as &'db <#jar_path as salsa::jar::Jar<'db>>::DynDb
                        }
                    }
                },

                parse_quote! {
                    impl salsa::storage::HasJar<#jar_path> for #db {
                        fn jar(&self) -> (&#jar_path, &salsa::Runtime) {
                            let (__jars, __runtime) = self.#storage.jars();
                            (&__jars.#jar_index, __runtime)
                        }

                        fn jar_mut(&mut self) -> (&mut #jar_path, &mut salsa::Runtime) {
                            let (__jars, __runtime) = self.#storage.jars_mut();
                            (&mut __jars.#jar_index, __runtime)
                        }
                    }
                },

                parse_quote! {
                    impl salsa::storage::JarFromJars<#jar_path> for #db {
                        fn jar_from_jars<'db>(jars: &Self::Jars) -> &#jar_path {
                            &jars.#jar_index
                        }

                        fn jar_from_jars_mut<'db>(jars: &mut Self::Jars) -> &mut #jar_path {
                            &mut jars.#jar_index
                        }
                    }
                }
            ]
        })
        .collect()
}
