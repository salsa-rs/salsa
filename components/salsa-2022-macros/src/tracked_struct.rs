use proc_macro2::{Literal, Span, TokenStream};

use crate::salsa_struct::{SalsaField, SalsaStruct, TheStructKind};

/// For an tracked struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the tracked ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn tracked(
    args: proc_macro::TokenStream,
    struct_item: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let struct_name = struct_item.ident.clone();

    let tokens = SalsaStruct::with_struct(args, struct_item, "tracked_struct")
        .and_then(|el| TrackedStruct(el).generate_tracked())?;

    Ok(crate::debug::dump_tokens(&struct_name, tokens))
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
        self.require_db_lifetime()?;

        let config_struct = self.config_struct();
        let the_struct = self.the_struct(&config_struct.ident)?;
        let config_impl = self.config_impl(&config_struct);
        let inherent_impl = self.tracked_inherent_impl();
        let ingredients_for_impl = self.tracked_struct_ingredients(&config_struct);
        let salsa_struct_in_db_impl = self.salsa_struct_in_db_impl();
        let tracked_struct_in_db_impl = self.tracked_struct_in_db_impl();
        let update_impl = self.update_impl();
        let as_id_impl = self.as_id_impl();
        let send_sync_impls = self.send_sync_impls();
        let from_id_impl = self.from_id_impl();
        let lookup_id_impl = self.lookup_id_impl();
        let debug_impl = self.debug_impl();
        let as_debug_with_db_impl = self.as_debug_with_db_impl();
        Ok(quote! {
            #config_struct
            #config_impl
            #the_struct
            #inherent_impl
            #ingredients_for_impl
            #salsa_struct_in_db_impl
            #tracked_struct_in_db_impl
            #update_impl
            #as_id_impl
            #from_id_impl
            #(#send_sync_impls)*
            #lookup_id_impl
            #as_debug_with_db_impl
            #debug_impl
        })
    }

    fn config_impl(&self, config_struct: &syn::ItemStruct) -> syn::ItemImpl {
        let config_ident = &config_struct.ident;
        let field_tys: Vec<_> = self.all_fields().map(SalsaField::ty).collect();
        let id_field_indices = self.id_field_indices();
        let arity = self.all_field_count();
        let the_ident = self.the_ident();
        let lt_db = &self.named_db_lifetime();

        // Create the function body that will update the revisions for each field.
        // If a field is a "backdate field" (the default), then we first check if
        // the new value is `==` to the old value. If so, we leave the revision unchanged.
        let old_fields = syn::Ident::new("old_fields_", Span::call_site());
        let new_fields = syn::Ident::new("new_fields_", Span::call_site());
        let revisions = syn::Ident::new("revisions_", Span::call_site());
        let current_revision = syn::Ident::new("current_revision_", Span::call_site());
        let update_fields: TokenStream = self
            .all_fields()
            .zip(0..)
            .map(|(field, i)| {
                let field_ty = field.ty();
                let field_index = Literal::u32_unsuffixed(i);
                if field.is_backdate_field() {
                    quote_spanned! { field.span() =>
                        if salsa::update::helper::Dispatch::<#field_ty>::maybe_update(
                            std::ptr::addr_of_mut!((*#old_fields).#field_index),
                            #new_fields.#field_index,
                        ) {
                            #revisions[#field_index] = #current_revision;
                        }
                    }
                } else {
                    quote_spanned! { field.span() =>
                        salsa::update::always_update(
                            &mut #revisions[#field_index],
                            #current_revision,
                            unsafe { &mut (*#old_fields).#field_index },
                            #new_fields.#field_index,
                        );
                    }
                }
            })
            .collect();

        parse_quote! {
            impl salsa::tracked_struct::Configuration for #config_ident {
                type Fields<#lt_db> = ( #(#field_tys,)* );

                type Struct<#lt_db> = #the_ident<#lt_db>;

                type Revisions = [salsa::Revision; #arity];

                unsafe fn struct_from_raw<'db>(ptr: std::ptr::NonNull<salsa::tracked_struct::ValueStruct<Self>>) -> Self::Struct<'db> {
                    #the_ident(ptr, std::marker::PhantomData)
                }

                fn deref_struct<'db>(s: Self::Struct<'db>) -> &'db salsa::tracked_struct::ValueStruct<Self> {
                    unsafe { s.0.as_ref() }
                }

                #[allow(clippy::unused_unit)]
                fn id_fields(fields: &Self::Fields<'_>) -> impl std::hash::Hash {
                    ( #( &fields.#id_field_indices ),* )
                }

                fn revision(revisions: &Self::Revisions, field_index: u32) -> salsa::Revision {
                    revisions[field_index as usize]
                }

                fn new_revisions(current_revision: salsa::Revision) -> Self::Revisions {
                    [current_revision; #arity]
                }

                unsafe fn update_fields<#lt_db>(
                    #current_revision: salsa::Revision,
                    #revisions: &mut Self::Revisions,
                    #old_fields: *mut Self::Fields<#lt_db>,
                    #new_fields: Self::Fields<#lt_db>,
                ) {
                    use salsa::update::helper::Fallback as _;
                    #update_fields
                }
            }
        }
    }

    /// Generate an inherent impl with methods on the tracked type.
    fn tracked_inherent_impl(&self) -> syn::ItemImpl {
        let (ident, _, impl_generics, type_generics, where_clause) = self.the_ident_and_generics();

        let the_kind = &self.the_struct_kind();

        let jar_ty = self.jar_ty();
        let db_dyn_ty = self.db_dyn_ty();
        let tracked_field_ingredients: Literal = self.tracked_field_ingredients_index();

        let field_indices = self.all_field_indices();
        let field_vises: Vec<_> = self.all_fields().map(SalsaField::vis).collect();
        let field_tys: Vec<_> = self.all_fields().map(SalsaField::ty).collect();
        let field_get_names: Vec<_> = self.all_fields().map(SalsaField::get_name).collect();
        let field_clones: Vec<_> = self.all_fields().map(SalsaField::is_clone_field).collect();
        let field_getters: Vec<syn::ImplItemFn> = field_indices.iter().zip(&field_get_names).zip(&field_tys).zip(&field_vises).zip(&field_clones).map(|((((field_index, field_get_name), field_ty), field_vis), is_clone_field)|
            match the_kind {
                TheStructKind::Id => {
                    if !*is_clone_field {
                        parse_quote_spanned! { field_get_name.span() =>
                            #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> &'db #field_ty
                            {
                                let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                                let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident #type_generics >>::ingredient(__jar);
                                &__ingredients.#tracked_field_ingredients[#field_index].field(__runtime, self.0).#field_index
                            }
                        }
                    } else {
                        parse_quote_spanned! { field_get_name.span() =>
                            #field_vis fn #field_get_name<'db>(self, __db: &'db #db_dyn_ty) -> #field_ty
                            {
                                let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                                let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< #ident #type_generics >>::ingredient(__jar);
                                __ingredients.#tracked_field_ingredients[#field_index].field(__runtime, self.0).#field_index.clone()
                            }
                        }
                    }
                }

                TheStructKind::Pointer(lt_db) => {
                    if !*is_clone_field {
                        parse_quote_spanned! { field_get_name.span() =>
                            #field_vis fn #field_get_name(self, __db: & #lt_db #db_dyn_ty) -> & #lt_db #field_ty
                            {
                                let (_, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                                let fields = unsafe { self.0.as_ref() }.field(__runtime, #field_index);
                                &fields.#field_index
                            }
                        }
                    } else {
                        parse_quote_spanned! { field_get_name.span() =>
                            #field_vis fn #field_get_name(self, __db: & #lt_db #db_dyn_ty) -> #field_ty
                            {
                                let (_, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                                let fields = unsafe { self.0.as_ref() }.field(__runtime, #field_index);
                                fields.#field_index.clone()
                            }
                        }
                    }
                }
            }
        )
        .collect();

        let field_names = self.all_field_names();
        let field_tys = self.all_field_tys();
        let constructor_name = self.constructor_name();

        let data = syn::Ident::new("__data", Span::call_site());

        let salsa_id = self.access_salsa_id_from_self();

        let lt_db = self.maybe_elided_db_lifetime();
        parse_quote! {
            #[allow(dead_code, clippy::pedantic, clippy::complexity, clippy::style)]
            impl #impl_generics #ident #type_generics
            #where_clause {
                pub fn #constructor_name(__db: &#lt_db #db_dyn_ty, #(#field_names: #field_tys,)*) -> Self
                {
                    let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                    let __ingredients = <#jar_ty as salsa::storage::HasIngredientsFor< Self >>::ingredient(__jar);
                    __ingredients.0.new_struct(
                        __runtime,
                        (#(#field_names,)*),
                    )
                }

                pub fn salsa_id(&self) -> salsa::Id {
                    #salsa_id
                }

                #(#field_getters)*
            }
        }
    }

    /// Generate the `IngredientsFor` impl for this tracked struct.
    ///
    /// The tracked struct's ingredients include both the main tracked struct ingredient along with a
    /// function ingredient for each of the value fields.
    fn tracked_struct_ingredients(&self, config_struct: &syn::ItemStruct) -> syn::ItemImpl {
        use crate::literal;
        let (ident, _, impl_generics, type_generics, where_clause) = self.the_ident_and_generics();
        let jar_ty = self.jar_ty();
        let config_struct_name = &config_struct.ident;
        let field_indices: Vec<Literal> = self.all_field_indices();
        let arity = self.all_field_count();
        let tracked_struct_ingredient: Literal = self.tracked_struct_ingredient_index();
        let tracked_fields_ingredients: Literal = self.tracked_field_ingredients_index();
        let debug_name_struct = literal(self.the_ident());
        let debug_name_fields: Vec<_> = self.all_field_names().into_iter().map(literal).collect();

        parse_quote! {
            impl #impl_generics salsa::storage::IngredientsFor for #ident #type_generics
            #where_clause {
                type Jar = #jar_ty;
                type Ingredients = (
                    salsa::tracked_struct::TrackedStructIngredient<#config_struct_name>,
                    [salsa::tracked_struct::TrackedFieldIngredient<#config_struct_name>; #arity],
                );

                fn create_ingredients<DB>(
                    routes: &mut salsa::routes::Routes<DB>,
                ) -> Self::Ingredients
                where
                    DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
                {
                    let struct_ingredient = {
                        let index = routes.push(
                            |jars| {
                                let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                &ingredients.#tracked_struct_ingredient
                            },
                            |jars| {
                                let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                                &mut ingredients.#tracked_struct_ingredient
                            },
                        );
                        salsa::tracked_struct::TrackedStructIngredient::new(index, #debug_name_struct)
                    };

                    let field_ingredients = [
                        #(
                            {
                                let index = routes.push(
                                    |jars| {
                                        let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                        let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                                        &ingredients.#tracked_fields_ingredients[#field_indices]
                                    },
                                    |jars| {
                                        let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                        let ingredients = <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                                        &mut ingredients.#tracked_fields_ingredients[#field_indices]
                                    },
                                );
                                struct_ingredient.new_field_ingredient(index, #field_indices, #debug_name_fields)
                            },
                        )*
                    ];

                    (struct_ingredient, field_ingredients)
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
                let tracked_struct_ingredient = self.tracked_struct_ingredient_index();
                Some(parse_quote_spanned! { ident.span() =>
                    impl<#db, #parameters> salsa::id::LookupId<& #db_lt #db> for #ident #type_generics
                    where
                        #db: ?Sized + salsa::DbWithJar<#jar_ty>,
                        #where_clause
                    {
                        fn lookup_id(id: salsa::Id, db: & #db_lt DB) -> Self {
                            let (jar, runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                            let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident #type_generics>>::ingredient(jar);
                            ingredients.#tracked_struct_ingredient.lookup_struct(runtime, id)
                        }
                    }
                })
            }
        }
    }

    /// Implementation of `SalsaStructInDb`.
    fn salsa_struct_in_db_impl(&self) -> syn::ItemImpl {
        let (ident, parameters, _, type_generics, where_clause) = self.the_ident_and_generics();
        let db = syn::Ident::new("DB", ident.span());
        let jar_ty = self.jar_ty();
        let tracked_struct_ingredient = self.tracked_struct_ingredient_index();
        parse_quote! {
            impl<#db, #parameters> salsa::salsa_struct::SalsaStructInDb<#db> for #ident #type_generics
            where
                #db: ?Sized + salsa::DbWithJar<#jar_ty>,
                #where_clause
            {
                fn register_dependent_fn(db: & #db, index: salsa::routes::IngredientIndex) {
                    let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                    let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident #type_generics>>::ingredient(jar);
                    ingredients.#tracked_struct_ingredient.register_dependent_fn(index)
                }
            }
        }
    }

    /// Implementation of `TrackedStructInDb`.
    fn tracked_struct_in_db_impl(&self) -> syn::ItemImpl {
        let (ident, parameters, _, type_generics, where_clause) = self.the_ident_and_generics();
        let db = syn::Ident::new("DB", ident.span());
        let jar_ty = self.jar_ty();
        let tracked_struct_ingredient = self.tracked_struct_ingredient_index();
        parse_quote! {
            impl<#db, #parameters> salsa::tracked_struct::TrackedStructInDb<#db> for #ident #type_generics
            where
                #db: ?Sized + salsa::DbWithJar<#jar_ty>,
                #where_clause
            {
                fn database_key_index(db: &#db, id: salsa::Id) -> salsa::DatabaseKeyIndex {
                    let (jar, _) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(db);
                    let ingredients = <#jar_ty as salsa::storage::HasIngredientsFor<#ident #type_generics>>::ingredient(jar);
                    ingredients.#tracked_struct_ingredient.database_key_index(id)
                }
            }
        }
    }

    /// The index of the tracked struct ingredient in the ingredient tuple.
    fn tracked_struct_ingredient_index(&self) -> Literal {
        Literal::usize_unsuffixed(0)
    }

    /// The index of the tracked field ingredients array in the ingredient tuple.
    fn tracked_field_ingredients_index(&self) -> Literal {
        Literal::usize_unsuffixed(1)
    }

    /// For this struct, we create a tuple that contains the function ingredients
    /// for each field and the tracked-struct ingredient. These are the indices
    /// of the function ingredients within that tuple.
    fn all_field_indices(&self) -> Vec<Literal> {
        (0..self.all_fields().count())
            .map(Literal::usize_unsuffixed)
            .collect()
    }

    /// For this struct, we create a tuple that contains the function ingredients
    /// for each "other" field and the tracked-struct ingredient. These are the indices
    /// of the function ingredients within that tuple.
    fn all_field_count(&self) -> Literal {
        Literal::usize_unsuffixed(self.all_fields().count())
    }

    /// Indices of each of the id fields
    fn id_field_indices(&self) -> Vec<Literal> {
        self.all_fields()
            .zip(0..)
            .filter(|(field, _)| field.is_id_field())
            .map(|(_, index)| Literal::usize_unsuffixed(index))
            .collect()
    }
}

impl SalsaField {
    /// true if this is an id field
    fn is_id_field(&self) -> bool {
        self.has_id_attr
    }
}
