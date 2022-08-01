use heck::CamelCase;
use proc_macro2::{Literal, TokenStream};

use crate::configuration;
use crate::entity_like::{EntityField, EntityLike};

// #[salsa::Entity(#id_ident in Jar0)]
// #[derive(Eq, PartialEq, Hash, Debug, Clone)]
// struct EntityData0 {
//    id: u32
// }

pub(crate) fn entity(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match EntityLike::new(args, input)
        .and_then(|el| el.generate_entity())
    {
        Ok(s) => s.into(),
        Err(err) => err.into_compile_error().into(),
    }
}

impl EntityLike {
    fn generate_entity(&self) -> syn::Result<TokenStream> {
        self.validate_entity()?;

        let config_structs = self.config_structs();

        let id_struct = self.id_struct();
        let inherent_impl = self.id_inherent_impl();
        let ingredients_for_impl = self.id_ingredients_for_impl(&config_structs);
        let entity_in_db_impl = self.entity_in_db_impl();
        let as_id_impl = self.as_id_impl();
        let config_impls = self.config_impls(&config_structs);

        Ok(quote! {
            #(#config_structs)*
            #id_struct
            #inherent_impl
            #ingredients_for_impl
            #entity_in_db_impl
            #as_id_impl
            #(#config_impls)*
        })
    }

    fn validate_entity(&self) -> syn::Result<()> {
        // Require that entities are structs for now.
        if !self.has_named_fields() {
            return Err(syn::Error::new(
                self.id_ident().span(),
                "entities must be structs with named fields",
            ));
        }

        Ok(())
    }

    /// For each of the value fields in the entity,
    /// we will generate a memoized function that stores its value.
    /// Generate a struct for the "Configuration" of each of those functions.
    fn config_structs(&self) -> Vec<syn::ItemStruct> {
        let ident = &self.id_ident();
        let visibility = self.visibility();
        self.value_fields()
            .map(EntityField::name)
            .map(|value_field_name| {
                let config_name = syn::Ident::new(
                    &format!(
                        "__{}",
                        format!("{}_{}", ident, value_field_name).to_camel_case()
                    ),
                    value_field_name.span(),
                );
                parse_quote! {
                    #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
                    #visibility struct #config_name(std::convert::Infallible);
                }
            })
            .collect()
    }

    /// Generate an inherent impl with methods on the entity type.
    fn id_inherent_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let db_dyn_ty = self.db_dyn_ty();
        let entity_index = self.entity_index();

        let id_field_indices: Vec<_> = self.id_field_indices();
        let id_field_names: Vec<_> = self.id_fields().map(EntityField::name).collect();
        let id_field_tys: Vec<_> = self.id_fields().map(EntityField::ty).collect();
        let id_field_clones: Vec<_> = self.id_fields().map(EntityField::is_clone_field).collect();
        let id_field_getters: Vec<syn::ImplItemMethod> = id_field_indices.iter().zip(&id_field_names).zip(&id_field_tys).zip(&id_field_clones).map(|(((field_index, field_name), field_ty), is_clone_field)|
            if !*is_clone_field {
                parse_quote! {
                    pub fn #field_name<'db>(self, __db: &'db #db_dyn_ty) -> &'db #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        &__ingredients.#entity_index.entity_data(__runtime, self).#field_index
                    }
                }
            } else {
                parse_quote! {
                    pub fn #field_name<'db>(self, __db: &'db #db_dyn_ty) -> #field_ty
                    {
                        let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                        let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                        __ingredients.#entity_index.entity_data(__runtime, self).#field_index.clone()
                    }
                }
            }
        )
        .collect();

        let value_field_indices = self.value_field_indices();
        let value_field_names: Vec<_> = self.value_fields().map(EntityField::name).collect();
        let value_field_tys: Vec<_> = self.value_fields().map(EntityField::ty).collect();
        let value_field_clones: Vec<_> = self
            .value_fields()
            .map(EntityField::is_clone_field)
            .collect();
        let value_field_getters: Vec<syn::ImplItemMethod> = value_field_indices.iter().zip(&value_field_names).zip(&value_field_tys).zip(&value_field_clones).map(|(((field_index, field_name), field_ty), is_clone_field)|
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

        let all_field_names = self.all_field_names();
        let all_field_tys = self.all_field_tys();

        parse_quote! {
            impl #ident {
                pub fn new(__db: &#db_dyn_ty, #(#all_field_names: #all_field_tys,)*) -> Self
                {
                    let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                    let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident >>::ingredient(__jar);
                    let __id = __ingredients.#entity_index.new_entity(__runtime, (#(#id_field_names,)*));
                    #(
                        __ingredients.#value_field_indices.set(__db, __id, #value_field_names);
                    )*
                    __id
                }

                #(#id_field_getters)*

                #(#value_field_getters)*
            }
        }
    }

    /// Generate the `IngredientsFor` impl for this entity.
    ///
    /// The entity's ingredients include both the main entity ingredient along with a
    /// function ingredient for each of the value fields.
    fn id_ingredients_for_impl(&self, config_structs: &[syn::ItemStruct]) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let id_field_tys: Vec<&syn::Type> = self.id_fields().map(EntityField::ty).collect();
        let value_field_indices: Vec<Literal> = self.value_field_indices();
        let entity_index: Literal = self.entity_index();
        let config_struct_names = config_structs.iter().map(|s| &s.ident);
        
        parse_quote! {
            impl salsa::storage::IngredientsFor for #ident {
                type Jar = #jar_ty;
                type Ingredients = (
                    #(
                        salsa::function::FunctionIngredient<#config_struct_names>,
                    )*
                    salsa::entity::EntityIngredient<#ident, (#(#id_field_tys,)*)>,
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
                                        &ingredients.#value_field_indices
                                    },
                                );
                                salsa::function::FunctionIngredient::new(index)
                            },
                        )*
                        {
                            let index = ingredients.push_mut(
                                |jars| {
                                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                    let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                    &ingredients.#entity_index
                                },
                                |jars| {
                                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                    let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                                    &mut ingredients.#entity_index
                                },
                            );
                            salsa::entity::EntityIngredient::new(index)
                        },
                    )
                }
            }
        }
    }

    /// Implementation of `EntityInDb` for this entity.
    fn entity_in_db_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let entity_index = self.entity_index();
        parse_quote! {
            impl<DB> salsa::entity::EntityInDb<DB> for #ident
            where
                DB: ?Sized + salsa::DbWithJar<#jar_ty>,
            {
                fn database_key_index(self, db: &DB) -> salsa::DatabaseKeyIndex {
                    let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                    let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident>>::ingredient(jar);
                    ingredients.#entity_index.database_key_index(self)
                }
            }
        }
    }

    fn config_impls(&self, config_structs: &[syn::ItemStruct]) -> Vec<syn::ItemImpl> {
        let ident = self.id_ident();
        let jar_ty = self.jar_ty();
        let value_field_tys = self.value_fields().map(EntityField::ty);
        let value_field_backdates = self.value_fields().map(EntityField::is_backdate_field);
        value_field_tys
        .into_iter()
        .zip(config_structs.iter().map(|s| &s.ident))
        .zip(value_field_backdates)
        .map(|((value_field_ty, config_struct_name), value_field_backdate)| {
            let should_backdate_value_fn = configuration::should_backdate_value_fn(value_field_backdate);
    
            parse_quote! {
                impl salsa::function::Configuration for #config_struct_name {
                    type Jar = #jar_ty;
                    type Key = #ident;
                    type Value = #value_field_ty;
                    const CYCLE_STRATEGY: salsa::cycle::CycleRecoveryStrategy = salsa::cycle::CycleRecoveryStrategy::Panic;
    
                    #should_backdate_value_fn
    
                    fn execute(db: &salsa::function::DynDb<Self>, key: Self::Key) -> Self::Value {
                        unreachable!()
                    }
                
                    fn recover_from_cycle(db: &salsa::function::DynDb<Self>, cycle: &salsa::Cycle, key: Self::Key) -> Self::Value {
                        unreachable!()
                    }
                }
            }
        })
        .collect()
    }
    
    /// List of id fields (fields that are part of the entity's identity across revisions).
    ///
    /// If this is an enum, empty iterator.
    fn id_fields(&self) -> impl Iterator<Item = &EntityField> {
        self.all_entity_fields().filter(|ef| ef.is_entity_id_field())
    }

    /// List of value fields (fields that are not part of the entity's identity across revisions).
    ///
    /// If this is an enum, empty iterator.
    fn value_fields(&self) -> impl Iterator<Item = &EntityField> {
        self.all_entity_fields().filter(|ef| !ef.is_entity_id_field())
    }

    /// For the entity, we create a tuple that contains the function ingredients
    /// for each "other" field and the entity ingredient. This is the index of
    /// the entity ingredient within that tuple.
    fn entity_index(&self) -> Literal {
        Literal::usize_unsuffixed(self.value_fields().count())
    }

    /// For the entity, we create a tuple that contains the function ingredients
    /// for each "other" field and the entity ingredient. These are the indices
    /// of the function ingredients within that tuple.
    fn value_field_indices(&self) -> Vec<Literal> {
        (0..self.value_fields().count())
            .map(|i| Literal::usize_unsuffixed(i))
            .collect()
    }

    /// Indices of each of the id fields
    fn id_field_indices(&self) -> Vec<Literal> {
        (0..self.id_fields().count())
            .map(|i| Literal::usize_unsuffixed(i))
            .collect()
    }
}

impl EntityField {
    /// true if this is an id field
    fn is_entity_id_field(&self) -> bool {
        self.has_id_attr
    }
}