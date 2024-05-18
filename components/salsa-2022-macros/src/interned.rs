use crate::salsa_struct::{SalsaStruct, TheStructKind};
use proc_macro2::TokenStream;

// #[salsa::interned(jar = Jar0, data = TyData0)]
// #[derive(Eq, PartialEq, Hash, Debug, Clone)]
// struct Ty0 {
//    field1: Type1,
//    #[id(ref)] field2: Type2,
//    ...
// }

pub(crate) fn interned(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match SalsaStruct::new(args, input, "interned")
        .and_then(|el| InternedStruct(el).generate_interned())
    {
        Ok(s) => s.into(),
        Err(err) => err.into_compile_error().into(),
    }
}

struct InternedStruct(SalsaStruct<Self>);

impl std::ops::Deref for InternedStruct {
    type Target = SalsaStruct<Self>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl crate::options::AllowedOptions for InternedStruct {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const SINGLETON: bool = false;

    const JAR: bool = true;

    const DATA: bool = true;

    const DB: bool = false;

    const RECOVERY_FN: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = true;
}

impl InternedStruct {
    fn generate_interned(&self) -> syn::Result<TokenStream> {
        self.validate_interned()?;
        let config_struct = self.config_struct();
        let the_struct = self.the_struct(&config_struct.ident)?;
        let data_struct = self.data_struct();
        let configuration_impl = self.configuration_impl(&data_struct.ident, &config_struct.ident);
        let ingredients_for_impl = self.ingredients_for_impl(&config_struct.ident);
        let as_id_impl = self.as_id_impl();
        let from_id_impl = self.from_id_impl();
        let lookup_id_impl = self.lookup_id_impl();
        let named_fields_impl = self.inherent_impl_for_named_fields();
        let salsa_struct_in_db_impl = self.salsa_struct_in_db_impl();
        let as_debug_with_db_impl = self.as_debug_with_db_impl();

        Ok(crate::debug::dump_tokens(
            self.the_ident(),
            quote! {
                #the_struct
                #config_struct
                #configuration_impl
                #data_struct
                #ingredients_for_impl
                #as_id_impl
                #from_id_impl
                #lookup_id_impl
                #named_fields_impl
                #salsa_struct_in_db_impl
                #as_debug_with_db_impl
            },
        ))
    }

    fn validate_interned(&self) -> syn::Result<()> {
        self.disallow_id_fields("interned")?;
        self.require_db_lifetime()?;
        Ok(())
    }

    /// The name of the "data" struct (this comes from the `data = Foo` option or,
    /// if that is not provided, by concatenating `Data` to the name of the struct).
    fn data_ident(&self) -> syn::Ident {
        match &self.args().data {
            Some(d) => d.clone(),
            None => syn::Ident::new(
                &format!("__{}Data", self.the_ident()),
                self.the_ident().span(),
            ),
        }
    }

    /// Generates the `struct FooData` struct (or enum).
    /// This type inherits all the attributes written by the user.
    ///
    /// When using named fields, we synthesize the struct and field names.
    ///
    /// When no named fields are available, copy the existing type.
    fn data_struct(&self) -> syn::ItemStruct {
        let data_ident = self.data_ident();
        let (_, _, impl_generics, _, where_clause) = self.the_ident_and_generics();

        let visibility = self.visibility();
        let all_field_names = self.all_field_names();
        let all_field_tys = self.all_field_tys();

        match self.the_struct_kind() {
            TheStructKind::Id => {
                parse_quote_spanned! { data_ident.span() =>
                    #[derive(Eq, PartialEq, Hash, Clone)]
                    #visibility struct #data_ident #impl_generics
                    where
                        #where_clause
                    {
                        #(
                            #all_field_names: #all_field_tys,
                        )*
                    }
                }
            }
            TheStructKind::Pointer(db_lt) => {
                parse_quote_spanned! { data_ident.span() =>
                    #[derive(Eq, PartialEq, Hash, Clone)]
                    #visibility struct #data_ident #impl_generics
                    where
                        #where_clause
                    {
                        #(
                            #all_field_names: #all_field_tys,
                        )*
                        __phantom: std::marker::PhantomData<& #db_lt ()>,
                    }
                }
            }
        }
    }

    fn configuration_impl(
        &self,
        data_ident: &syn::Ident,
        config_ident: &syn::Ident,
    ) -> syn::ItemImpl {
        let lt_db = &self.named_db_lifetime();
        let (_, _, _, type_generics, _) = self.the_ident_and_generics();
        parse_quote_spanned!(
            config_ident.span() =>

            impl salsa::interned::Configuration for #config_ident {
                type Data<#lt_db> = #data_ident #type_generics;
            }
        )
    }

    /// If this is an interned struct, then generate methods to access each field,
    /// as well as a `new` method.
    fn inherent_impl_for_named_fields(&self) -> syn::ItemImpl {
        match self.the_struct_kind() {
            TheStructKind::Id => self.inherent_impl_for_named_fields_id(),
            TheStructKind::Pointer(db_lt) => self.inherent_impl_for_named_fields_lt(&db_lt),
        }
    }

    /// If this is an interned struct, then generate methods to access each field,
    /// as well as a `new` method.
    fn inherent_impl_for_named_fields_lt(&self, db_lt: &syn::Lifetime) -> syn::ItemImpl {
        let vis: &syn::Visibility = self.visibility();
        let (the_ident, _, impl_generics, type_generics, where_clause) =
            self.the_ident_and_generics();
        let db_dyn_ty = self.db_dyn_ty();
        let jar_ty = self.jar_ty();

        let field_getters: Vec<syn::ImplItemMethod> = self
            .all_fields()
            .map(|field| {
                let field_name = field.name();
                let field_ty = field.ty();
                let field_vis = field.vis();
                let field_get_name = field.get_name();
                if field.is_clone_field() {
                    parse_quote_spanned! { field_get_name.span() =>
                        #field_vis fn #field_get_name(self, _db: & #db_lt #db_dyn_ty) -> #field_ty {
                            std::clone::Clone::clone(&unsafe { &*self.0 }.data().#field_name)
                        }
                    }
                } else {
                    parse_quote_spanned! { field_get_name.span() =>
                        #field_vis fn #field_get_name(self, _db: & #db_lt #db_dyn_ty) -> & #db_lt #field_ty {
                            &unsafe { &*self.0 }.data().#field_name
                        }
                    }
                }
            })
            .collect();

        let field_names = self.all_field_names();
        let field_tys = self.all_field_tys();
        let data_ident = self.data_ident();
        let constructor_name = self.constructor_name();
        let new_method: syn::ImplItemMethod = parse_quote_spanned! { constructor_name.span() =>
            #vis fn #constructor_name(
                db: &#db_dyn_ty,
                #(#field_names: #field_tys,)*
            ) -> Self {
                let (jar, runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #the_ident #type_generics >>::ingredient(jar);
                Self(
                    ingredients.intern(runtime, #data_ident {
                        #(#field_names,)*
                        __phantom: std::marker::PhantomData,
                    }),
                    std::marker::PhantomData,
                )
            }
        };

        let salsa_id = quote!(
            pub fn salsa_id(&self) -> salsa::Id {
                salsa::id::AsId::as_id(unsafe { &*self })
            }
        );

        parse_quote! {
            #[allow(dead_code, clippy::pedantic, clippy::complexity, clippy::style)]
            impl #impl_generics #the_ident #type_generics
            where
                #where_clause
            {
                #(#field_getters)*

                #new_method

                #salsa_id
            }
        }
    }

    /// If this is an interned struct, then generate methods to access each field,
    /// as well as a `new` method.
    fn inherent_impl_for_named_fields_id(&self) -> syn::ItemImpl {
        let vis: &syn::Visibility = self.visibility();
        let (the_ident, _, impl_generics, type_generics, where_clause) =
            self.the_ident_and_generics();
        let db_dyn_ty = self.db_dyn_ty();
        let jar_ty = self.jar_ty();

        let field_getters: Vec<syn::ImplItemMethod> = self
            .all_fields()
            .map(|field| {
                let field_name = field.name();
                let field_ty = field.ty();
                let field_vis = field.vis();
                let field_get_name = field.get_name();
                if field.is_clone_field() {
                    parse_quote_spanned! { field_get_name.span() =>
                        #field_vis fn #field_get_name(self, db: &#db_dyn_ty) -> #field_ty {
                            let (jar, _runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #the_ident #type_generics >>::ingredient(jar);
                            std::clone::Clone::clone(&ingredients.data(self.0).#field_name)
                        }
                    }
                } else {
                    parse_quote_spanned! { field_get_name.span() =>
                        #field_vis fn #field_get_name<'db>(self, db: &'db #db_dyn_ty) -> &'db #field_ty {
                            let (jar, _runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #the_ident #type_generics >>::ingredient(jar);
                            &ingredients.data(self.0).#field_name
                        }
                    }
                }
            })
            .collect();

        let field_names = self.all_field_names();
        let field_tys = self.all_field_tys();
        let data_ident = self.data_ident();
        let constructor_name = self.constructor_name();
        let new_method: syn::ImplItemMethod = parse_quote_spanned! { constructor_name.span() =>
            #vis fn #constructor_name(
                db: &#db_dyn_ty,
                #(#field_names: #field_tys,)*
            ) -> Self {
                let (jar, runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #the_ident #type_generics >>::ingredient(jar);
                salsa::id::FromId::from_as_id(ingredients.intern(runtime, #data_ident {
                    #(#field_names,)*
                }))
            }
        };

        let salsa_id = quote!(
            pub fn salsa_id(&self) -> salsa::Id {
                self.0
            }
        );

        parse_quote! {
            #[allow(dead_code, clippy::pedantic, clippy::complexity, clippy::style)]
            impl #impl_generics #the_ident #type_generics
            where
                #where_clause
            {
                #(#field_getters)*

                #new_method

                #salsa_id
            }
        }
    }

    /// Generates an impl of `salsa::storage::IngredientsFor`.
    ///
    /// For a memoized type, the only ingredient is an `InternedIngredient`.
    fn ingredients_for_impl(&self, config_ident: &syn::Ident) -> syn::ItemImpl {
        let (the_ident, _, impl_generics, type_generics, where_clause) =
            self.the_ident_and_generics();
        let debug_name = crate::literal(the_ident);
        let jar_ty = self.jar_ty();
        parse_quote! {
            impl #impl_generics salsa::storage::IngredientsFor for #the_ident #type_generics
            where
                #where_clause
            {
                type Jar = #jar_ty;
                type Ingredients = salsa::interned::InternedIngredient<#config_ident>;

                fn create_ingredients<DB>(
                    routes: &mut salsa::routes::Routes<DB>,
                ) -> Self::Ingredients
                where
                    DB: salsa::storage::JarFromJars<Self::Jar>,
                {
                    let index = routes.push(
                        |jars| {
                            let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar)
                        },
                        |jars| {
                            let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar)
                        },
                    );
                    salsa::interned::InternedIngredient::new(index, #debug_name)
                }
            }
        }
    }

    /// Implementation of `LookupId`.
    fn lookup_id_impl(&self) -> Option<syn::ItemImpl> {
        match self.the_struct_kind() {
            TheStructKind::Id => None,
            TheStructKind::Pointer(db_lt) => {
                let (ident, parameters, _, type_generics, where_clause) =
                    self.the_ident_and_generics();
                let db = syn::Ident::new("DB", ident.span());
                let jar_ty = self.jar_ty();
                Some(parse_quote_spanned! { ident.span() =>
                    impl<#db, #parameters> salsa::id::LookupId<& #db_lt #db> for #ident #type_generics
                    where
                        #db: ?Sized + salsa::DbWithJar<#jar_ty>,
                        #where_clause
                    {
                        fn lookup_id(id: salsa::Id, db: & #db_lt DB) -> Self {
                            let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident #type_generics>>::ingredient(jar);
                            Self(ingredients.interned_value(id), std::marker::PhantomData)
                        }
                    }
                })
            }
        }
    }

    /// Implementation of `SalsaStructInDb`.
    fn salsa_struct_in_db_impl(&self) -> syn::ItemImpl {
        let (the_ident, parameters, _, type_generics, where_clause) = self.the_ident_and_generics();
        #[allow(non_snake_case)]
        let DB = syn::Ident::new("DB", the_ident.span());
        let jar_ty = self.jar_ty();
        parse_quote! {
            impl<#DB, #parameters> salsa::salsa_struct::SalsaStructInDb<DB> for #the_ident #type_generics
            where
                #DB: ?Sized + salsa::DbWithJar<#jar_ty>,
                #where_clause
            {
                fn register_dependent_fn(_db: &#DB, _index: salsa::routes::IngredientIndex) {
                    // Do nothing here, at least for now.
                    // If/when we add ability to delete inputs, this would become relevant.
                }
            }
        }
    }
}
