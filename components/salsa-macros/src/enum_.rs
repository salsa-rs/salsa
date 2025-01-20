use crate::token_stream_with_error;
use proc_macro2::TokenStream;

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn enum_(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let enum_item = parse_macro_input!(input as syn::ItemEnum);
    match enum_impl(enum_item) {
        Ok(v) => v.into(),
        Err(e) => token_stream_with_error(input, e),
    }
}

fn enum_impl(enum_item: syn::ItemEnum) -> syn::Result<TokenStream> {
    let enum_name = enum_item.ident.clone();
    let mut variant_names = Vec::new();
    let mut variant_types = Vec::new();
    if enum_item.variants.is_empty() {
        return Err(syn::Error::new(
            enum_item.enum_token.span,
            "empty enums are not permitted",
        ));
    }
    for variant in &enum_item.variants {
        let valid = match &variant.fields {
            syn::Fields::Unnamed(fields) => {
                variant_names.push(variant.ident.clone());
                variant_types.push(fields.unnamed[0].ty.clone());
                fields.unnamed.len() == 1
            }
            syn::Fields::Unit | syn::Fields::Named(_) => false,
        };
        if !valid {
            return Err(syn::Error::new(
                variant.ident.span(),
                "the only form allowed is `Variant(SalsaStruct)`",
            ));
        }
    }

    let (impl_generics, type_generics, where_clause) = enum_item.generics.split_for_impl();

    let as_id = quote! {
        impl #impl_generics zalsa::AsId for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn as_id(&self) -> zalsa::Id {
                match self {
                    #( Self::#variant_names(__v) => zalsa::AsId::as_id(__v), )*
                }
            }
        }
    };

    let from_id = quote! {
        impl #impl_generics zalsa::FromIdWithDb for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn from_id(__id: zalsa::Id, __db: &(impl ?Sized + zalsa::Database)) -> Self {
                let __zalsa = __db.zalsa();
                let __type_id = __zalsa.lookup_page_type_id(__id);
                <Self as zalsa::SalsaStructInDb>::cast(__id, __type_id).expect("invalid enum variant")
            }
        }
    };

    let salsa_struct_in_db = quote! {
        impl #impl_generics zalsa::SalsaStructInDb for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn lookup_or_create_ingredient_index(__zalsa: &zalsa::Zalsa) -> zalsa::IngredientIndices {
                let mut __result = zalsa::IngredientIndices::uninitialized();
                #(
                    __result.merge(
                        &<#variant_types as zalsa::SalsaStructInDb>::lookup_or_create_ingredient_index(__zalsa)
                    );
                )*
                __result
            }

            #[inline]
            fn cast(id: zalsa::Id, type_id: ::core::any::TypeId) -> Option<Self> {
                #(
                    // Subtle: the ingredient can be missing, but in this case the id cannot come
                    // from it - because it wasn't initialized yet.
                    if let Some(result) = <#variant_types as zalsa::SalsaStructInDb>::cast(id, type_id) {
                        Some(Self::#variant_names(result))
                    } else
                )*
                {
                    None
                }
            }
        }
    };

    let std_traits = quote! {
        impl #impl_generics ::core::marker::Copy for #enum_name #type_generics
        #where_clause {}

        impl #impl_generics ::core::clone::Clone for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn clone(&self) -> Self { *self }
        }

        impl #impl_generics ::core::cmp::Eq for #enum_name #type_generics
        #where_clause {}

        impl #impl_generics ::core::cmp::PartialEq for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn eq(&self, __other: &Self) -> bool {
                zalsa::AsId::as_id(self) == zalsa::AsId::as_id(__other)
            }
        }

        impl #impl_generics ::core::hash::Hash for #enum_name #type_generics
        #where_clause {
            #[inline]
            fn hash<__H: ::core::hash::Hasher>(&self, __state: &mut __H) {
                ::core::hash::Hash::hash(&zalsa::AsId::as_id(self), __state);
            }
        }
    };

    let all_impls = quote! {
        const _: () = {
            use salsa::plumbing as zalsa;

            #as_id
            #from_id
            #salsa_struct_in_db

            // #std_traits
        };
    };
    Ok(all_impls)
}
