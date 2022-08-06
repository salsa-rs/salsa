use proc_macro2::{Literal, TokenStream};

use crate::salsa_struct::{SalsaField, SalsaStruct};

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn input(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match SalsaStruct::new(args, input).and_then(|el| el.generate_input()) {
        Ok(s) => s.into(),
        Err(err) => err.into_compile_error().into(),
    }
}

impl SalsaStruct {
    fn generate_input(&self) -> syn::Result<TokenStream> {
        self.validate_input()?;

        let (config_structs, config_impls) = self.field_config_structs_and_impls(self.all_fields());

        let id_struct = self.id_struct();
        let inherent_impl = self.input_inherent_impl();
        let ingredients_for_impl = self.input_ingredients(&config_structs);
        let as_id_impl = self.as_id_impl();

        Ok(quote! {
            #(#config_structs)*
            #id_struct
            #inherent_impl
            #ingredients_for_impl
            #as_id_impl
            #(#config_impls)*
        })
    }

    fn validate_input(&self) -> syn::Result<()> {
        self.disallow_id_fields("input")?;

        Ok(())
    }

    /// Generate an inherent impl with methods on the entity type.
    fn input_inherent_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let db_dyn_ty = self.db_dyn_ty();
        let input_index = self.input_index();

        let field_indices = self.all_field_indices();
        let field_names: Vec<_> = self.all_field_names();
        let field_tys: Vec<_> = self.all_field_tys();
        let field_clones: Vec<_> = self.all_fields().map(SalsaField::is_clone_field).collect();
        let field_getters: Vec<syn::ImplItemMethod> = field_indices.iter().zip(&field_names).zip(&field_tys).zip(&field_clones).map(|(((field_index, field_name), field_ty), is_clone_field)|
            if !*is_clone_field {
                parse_quote! {
                    pub fn #field_name<'db>(self, __db: &'db #db_dyn_ty) -> &'db #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#field_index.fetch(__db, self)
                    }
                }
            } else {
                parse_quote! {
                    pub fn #field_name<'db>(self, __db: &'db #db_dyn_ty) -> #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#field_index.fetch(__db, self).clone()
                    }
                }
            }
        )
        .collect();

        let field_setters: Vec<syn::ImplItemMethod> = field_indices.iter().zip(&field_names).zip(&field_tys).map(|((field_index, field_name), field_ty)| {
            let set_field_name = syn::Ident::new(&format!("set_{}", field_name), field_name.span());
            parse_quote! {
                pub fn #set_field_name<'db>(self, __db: &'db mut #db_dyn_ty, __value: #field_ty)
                {
                    let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar_mut(__db);
                    let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient_mut(__jar);
                    __ingredients.#field_index.store(__runtime, self, __value, salsa::Durability::LOW);
                }
            }
        })
        .collect();

        parse_quote! {
            impl #ident {
                pub fn new(__db: &mut #db_dyn_ty, #(#field_names: #field_tys,)*) -> Self
                {
                    let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar_mut(__db);
                    let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient_mut(__jar);
                    let __id = __ingredients.#input_index.new_input(__runtime);
                    #(
                        __ingredients.#field_indices.store(__runtime, __id, #field_names, salsa::Durability::LOW);
                    )*
                    __id
                }

                #(#field_getters)*

                #(#field_setters)*
            }
        }
    }

    /// Generate the `IngredientsFor` impl for this entity.
    ///
    /// The entity's ingredients include both the main entity ingredient along with a
    /// function ingredient for each of the value fields.
    fn input_ingredients(&self, config_structs: &[syn::ItemStruct]) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let all_field_indices: Vec<Literal> = self.all_field_indices();
        let input_index: Literal = self.input_index();
        let config_struct_names = config_structs.iter().map(|s| &s.ident);

        parse_quote! {
            impl salsa::storage::IngredientsFor for #ident {
                type Jar = #jar_ty;
                type Ingredients = (
                    #(
                        salsa::function::FunctionIngredient<#config_struct_names>,
                    )*
                    salsa::input::InputIngredient<#ident>,
                );

                fn create_ingredients<DB>(
                    ingredients: &mut salsa::routes::Ingredients<DB>,
                ) -> Self::Ingredients
                where
                    DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
                {
                    (
                        #(
                            {
                                let index = ingredients.push(
                                    |jars| {
                                        let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                        let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                        &ingredients.#all_field_indices
                                    },
                                );
                                salsa::function::FunctionIngredient::new(index)
                            },
                        )*
                        {
                            let index = ingredients.push(
                                |jars| {
                                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                    let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                    &ingredients.#input_index
                                },
                            );
                            salsa::input::InputIngredient::new(index)
                        },
                    )
                }
            }
        }
    }

    /// For the entity, we create a tuple that contains the function ingredients
    /// for each "other" field and the entity ingredient. This is the index of
    /// the entity ingredient within that tuple.
    fn input_index(&self) -> Literal {
        Literal::usize_unsuffixed(self.all_fields().count())
    }

    /// For the entity, we create a tuple that contains the function ingredients
    /// for each field and an entity ingredient. These are the indices
    /// of the function ingredients within that tuple.
    fn all_field_indices(&self) -> Vec<Literal> {
        self.all_fields()
            .zip(0..)
            .map(|(_, i)| Literal::usize_unsuffixed(i))
            .collect()
    }
}
