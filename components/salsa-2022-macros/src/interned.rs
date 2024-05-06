use crate::salsa_struct::SalsaStruct;
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
    match SalsaStruct::new(args, input).and_then(|el| InternedStruct(el).generate_interned()) {
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
        let id_struct = self.the_struct_id();
        let config_struct = self.config_struct();
        let data_struct = self.data_struct();
        let configuration_impl = self.configuration_impl(&data_struct.ident, &config_struct.ident);
        let ingredients_for_impl = self.ingredients_for_impl(&config_struct.ident);
        let as_id_impl = self.as_id_impl();
        let named_fields_impl = self.inherent_impl_for_named_fields();
        let salsa_struct_in_db_impl = self.salsa_struct_in_db_impl();
        let as_debug_with_db_impl = self.as_debug_with_db_impl();

        Ok(quote! {
            #id_struct
            #config_struct
            #configuration_impl
            #data_struct
            #ingredients_for_impl
            #as_id_impl
            #named_fields_impl
            #salsa_struct_in_db_impl
            #as_debug_with_db_impl
        })
    }

    fn validate_interned(&self) -> syn::Result<()> {
        self.disallow_id_fields("interned")?;
        self.require_no_generics()?;
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
        let ident = self.data_ident();
        let visibility = self.visibility();
        let all_field_names = self.all_field_names();
        let all_field_tys = self.all_field_tys();
        parse_quote_spanned! { ident.span() =>
            /// Internal struct used for interned item
            #[derive(Eq, PartialEq, Hash, Clone)]
            #visibility struct #ident {
                #(
                    #all_field_names: #all_field_tys,
                )*
            }
        }
    }

    fn configuration_impl(
        &self,
        data_struct: &syn::Ident,
        config_struct: &syn::Ident,
    ) -> syn::ItemImpl {
        parse_quote_spanned!(
            config_struct.span() =>

            impl salsa::interned::Configuration for #config_struct {
                type Data = #data_struct;
            }
        )
    }

    /// If this is an interned struct, then generate methods to access each field,
    /// as well as a `new` method.
    fn inherent_impl_for_named_fields(&self) -> syn::ItemImpl {
        let vis = self.visibility();
        let id_ident = self.the_ident();
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
                            let (jar, runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #id_ident >>::ingredient(jar);
                            std::clone::Clone::clone(&ingredients.data(runtime, self.0).#field_name)
                        }
                    }
                } else {
                    parse_quote_spanned! { field_get_name.span() =>
                        #field_vis fn #field_get_name<'db>(self, db: &'db #db_dyn_ty) -> &'db #field_ty {
                            let (jar, runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #id_ident >>::ingredient(jar);
                            &ingredients.data(runtime, self.0).#field_name
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
                let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #id_ident >>::ingredient(jar);
                Self(ingredients.intern(runtime, #data_ident {
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
            impl #id_ident {
                #(#field_getters)*

                #new_method

                #salsa_id
            }
        }
    }

    /// Generates an impl of `salsa::storage::IngredientsFor`.
    ///
    /// For a memoized type, the only ingredient is an `InternedIngredient`.
    fn ingredients_for_impl(&self, config_struct: &syn::Ident) -> syn::ItemImpl {
        let id_ident = self.the_ident();
        let debug_name = crate::literal(id_ident);
        let jar_ty = self.jar_ty();
        parse_quote! {
            impl salsa::storage::IngredientsFor for #id_ident {
                type Jar = #jar_ty;
                type Ingredients = salsa::interned::InternedIngredient<#config_struct>;

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

    /// Implementation of `SalsaStructInDb`.
    fn salsa_struct_in_db_impl(&self) -> syn::ItemImpl {
        let ident = self.the_ident();
        let jar_ty = self.jar_ty();
        parse_quote! {
            impl<DB> salsa::salsa_struct::SalsaStructInDb<DB> for #ident
            where
                DB: ?Sized + salsa::DbWithJar<#jar_ty>,
            {
                fn register_dependent_fn(_db: &DB, _index: salsa::routes::IngredientIndex) {
                    // Do nothing here, at least for now.
                    // If/when we add ability to delete inputs, this would become relevant.
                }
            }
        }
    }
}
