use proc_macro2::{Literal, TokenStream};

use crate::salsa_struct::{SalsaField, SalsaStruct, SalsaStructKind};

/// For an tracked struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the tracked ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn tracked(
    args: proc_macro::TokenStream,
    struct_item: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    SalsaStruct::with_struct(SalsaStructKind::Tracked, args, struct_item)
        .and_then(|el| TrackedStruct(el).generate_tracked())
}

struct TrackedStruct(SalsaStruct<Self>);

impl std::ops::Deref for TrackedStruct {
    type Target = SalsaStruct<Self>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl crate::options::AllowedOptions for TrackedStruct {
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

impl TrackedStruct {
    fn generate_tracked(&self) -> syn::Result<TokenStream> {
        self.validate_tracked()?;

        let (config_structs, config_impls) =
            self.field_config_structs_and_impls(self.value_fields());

        let id_struct = self.id_struct();
        let inherent_impl = self.tracked_inherent_impl();
        let ingredients_for_impl = self.tracked_struct_ingredients(&config_structs);
        let salsa_struct_in_db_impl = self.salsa_struct_in_db_impl();
        let tracked_struct_in_db_impl = self.tracked_struct_in_db_impl();
        let as_id_impl = self.as_id_impl();
        let as_debug_with_db_impl = self.as_debug_with_db_impl();
        Ok(quote! {
            #(#config_structs)*
            #id_struct
            #inherent_impl
            #ingredients_for_impl
            #salsa_struct_in_db_impl
            #tracked_struct_in_db_impl
            #as_id_impl
            #as_debug_with_db_impl
            #(#config_impls)*
        })
    }

    fn validate_tracked(&self) -> syn::Result<()> {
        Ok(())
    }

    /// Generate an inherent impl with methods on the tracked type.
    fn tracked_inherent_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let db_dyn_ty = self.db_dyn_ty();
        let struct_index = self.tracked_struct_index();

        let id_field_indices: Vec<_> = self.id_field_indices();
        let id_field_names: Vec<_> = self.id_fields().map(SalsaField::name).collect();
        let id_field_get_names: Vec<_> = self.id_fields().map(SalsaField::get_name).collect();
        let id_field_tys: Vec<_> = self.id_fields().map(SalsaField::ty).collect();
        let id_field_vises: Vec<_> = self.id_fields().map(SalsaField::vis).collect();
        let id_field_clones: Vec<_> = self.id_fields().map(SalsaField::is_clone_field).collect();
        let id_field_getters: Vec<syn::ImplItemMethod> = id_field_indices.iter().zip(&id_field_get_names).zip(&id_field_tys).zip(&id_field_vises).zip(&id_field_clones).map(|((((field_index, field_get_name), field_ty), field_vis), is_clone_field)|
            if !*is_clone_field {
                parse_quote! {
                    #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> &'db #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        &__ingredients.#struct_index.tracked_struct_data(__runtime, self).#field_index
                    }
                }
            } else {
                parse_quote! {
                    #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#struct_index.tracked_struct_data(__runtime, self).#field_index.clone()
                    }
                }
            }
        )
        .collect();

        let value_field_indices = self.value_field_indices();
        let value_field_names: Vec<_> = self.value_fields().map(SalsaField::name).collect();
        let value_field_vises: Vec<_> = self.value_fields().map(SalsaField::vis).collect();
        let value_field_tys: Vec<_> = self.value_fields().map(SalsaField::ty).collect();
        let value_field_get_names: Vec<_> = self.value_fields().map(SalsaField::get_name).collect();
        let value_field_clones: Vec<_> = self
            .value_fields()
            .map(SalsaField::is_clone_field)
            .collect();
        let value_field_getters: Vec<syn::ImplItemMethod> = value_field_indices.iter().zip(&value_field_get_names).zip(&value_field_tys).zip(&value_field_vises).zip(&value_field_clones).map(|((((field_index, field_get_name), field_ty), field_vis), is_clone_field)|
            if !*is_clone_field {
                parse_quote! {
                    #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> &'db #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#field_index.fetch(__db, self)
                    }
                }
            } else {
                parse_quote! {
                    #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#field_index.fetch(__db, self).clone()
                    }
                }
            }
        )
        .collect();

        let all_field_names = self.all_field_names();
        let all_field_tys = self.all_field_tys();
        let constructor_name = self.constructor_name();

        parse_quote! {
            impl #ident {
                pub fn #constructor_name(__db: &#db_dyn_ty, #(#all_field_names: #all_field_tys,)*) -> Self
                {
                    let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                    let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                    let __id = __ingredients.#struct_index.new_struct(__runtime, (#(#id_field_names,)*));
                    #(
                        __ingredients.#value_field_indices.specify_and_record(__db, __id, #value_field_names);
                    )*
                    __id
                }

                #(#id_field_getters)*

                #(#value_field_getters)*
            }
        }
    }

    /// Generate the `IngredientsFor` impl for this tracked struct.
    ///
    /// The tracked struct's ingredients include both the main tracked struct ingredient along with a
    /// function ingredient for each of the value fields.
    fn tracked_struct_ingredients(&self, config_structs: &[syn::ItemStruct]) -> syn::ItemImpl {
        use crate::literal;
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let id_field_tys: Vec<&syn::Type> = self.id_fields().map(SalsaField::ty).collect();
        let value_field_indices: Vec<Literal> = self.value_field_indices();
        let tracked_struct_index: Literal = self.tracked_struct_index();
        let config_struct_names = config_structs.iter().map(|s| &s.ident);
        let debug_name_struct = literal(self.id_ident());
        let debug_name_fields: Vec<_> = self.all_field_names().into_iter().map(literal).collect();

        parse_quote! {
            impl salsa::storage::IngredientsFor for #ident {
                type Jar = #jar_ty;
                type Ingredients = (
                    #(
                        salsa::function::FunctionIngredient<#config_struct_names>,
                    )*
                    salsa::tracked_struct::TrackedStructIngredient<#ident, (#(#id_field_tys,)*)>,
                );

                fn create_ingredients<DB>(
                    routes: &mut salsa::routes::Routes<DB>,
                ) -> Self::Ingredients
                where
                    DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
                {
                    (
                        #(
                            {
                                let index = routes.push(
                                    |jars| {
                                        let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                        let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                        &ingredients.#value_field_indices
                                    },
                                    |jars| {
                                        let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                        let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                                        &mut ingredients.#value_field_indices
                                    },
                                );
                                salsa::function::FunctionIngredient::new(index, #debug_name_fields)
                            },
                        )*
                        {
                            let index = routes.push(
                                |jars| {
                                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                    let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                    &ingredients.#tracked_struct_index
                                },
                                |jars| {
                                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                    let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                                    &mut ingredients.#tracked_struct_index
                                },
                            );
                            salsa::tracked_struct::TrackedStructIngredient::new(index, #debug_name_struct)
                        },
                    )
                }
            }
        }
    }

    /// Implementation of `SalsaStructInDb`.
    fn salsa_struct_in_db_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let tracked_struct_index: Literal = self.tracked_struct_index();
        parse_quote! {
            impl<DB> salsa::salsa_struct::SalsaStructInDb<DB> for #ident
            where
                DB: ?Sized + salsa::DbWithJar<#jar_ty>,
            {
                fn register_dependent_fn(db: &DB, index: salsa::routes::IngredientIndex) {
                    let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                    let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident>>::ingredient(jar);
                    ingredients.#tracked_struct_index.register_dependent_fn(index)
                }
            }
        }
    }

    /// Implementation of `TrackedStructInDb`.
    fn tracked_struct_in_db_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let tracked_struct_index = self.tracked_struct_index();
        parse_quote! {
            impl<DB> salsa::tracked_struct::TrackedStructInDb<DB> for #ident
            where
                DB: ?Sized + salsa::DbWithJar<#jar_ty>,
            {
                fn database_key_index(self, db: &DB) -> salsa::DatabaseKeyIndex {
                    let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                    let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident>>::ingredient(jar);
                    ingredients.#tracked_struct_index.database_key_index(self)
                }
            }
        }
    }

    /// List of id fields (fields that are part of the tracked struct's identity across revisions).
    ///
    /// If this is an enum, empty iterator.
    fn id_fields(&self) -> impl Iterator<Item = &SalsaField> {
        self.all_fields().filter(|ef| ef.is_id_field())
    }

    /// List of value fields (fields that are not part of the tracked struct's identity across revisions).
    ///
    /// If this is an enum, empty iterator.
    fn value_fields(&self) -> impl Iterator<Item = &SalsaField> {
        self.all_fields().filter(|ef| !ef.is_id_field())
    }

    /// For this struct, we create a tuple that contains the function ingredients
    /// for each "other" field and the tracked-struct ingredient. This is the index of
    /// the entity ingredient within that tuple.
    fn tracked_struct_index(&self) -> Literal {
        Literal::usize_unsuffixed(self.value_fields().count())
    }

    /// For this struct, we create a tuple that contains the function ingredients
    /// for each "other" field and the tracked-struct ingredient. These are the indices
    /// of the function ingredients within that tuple.
    fn value_field_indices(&self) -> Vec<Literal> {
        (0..self.value_fields().count())
            .map(Literal::usize_unsuffixed)
            .collect()
    }

    /// Indices of each of the id fields
    fn id_field_indices(&self) -> Vec<Literal> {
        (0..self.id_fields().count())
            .map(Literal::usize_unsuffixed)
            .collect()
    }
}

impl SalsaField {
    /// true if this is an id field
    fn is_id_field(&self) -> bool {
        self.has_id_attr
    }
}
