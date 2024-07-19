use proc_macro2::{Literal, TokenStream};
use synstructure::BindStyle;

use crate::hygiene::Hygiene;

pub(crate) fn update_derive(input: syn::DeriveInput) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&input);

    if let syn::Data::Union(_) = &input.data {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`derive(Update)` does not support `union`",
        ));
    }

    let mut structure = synstructure::Structure::new(&input);

    for v in structure.variants_mut() {
        v.bind_with(|_| BindStyle::Move);
    }

    let old_pointer = hygiene.ident("old_pointer");
    let new_value = hygiene.ident("new_value");

    let fields: TokenStream = structure
        .variants()
        .iter()
        .map(|variant| {
            let variant_pat = variant.pat();

            // First check that the `new_value` has same variant.
            // Extract its fields and convert to a tuple.
            let make_tuple = variant
                .bindings()
                .iter()
                .fold(quote!(), |tokens, binding| quote!(#tokens #binding,));
            let make_new_value = quote! {
                let #new_value = if let #variant_pat = #new_value {
                    (#make_tuple)
                } else {
                    *#old_pointer = #new_value;
                    return true;
                };
            };

            // For each field, invoke `maybe_update` recursively to update its value.
            // Or the results together (using `|`, not `||`, to avoid shortcircuiting)
            // to get the final return value.
            let update_fields = variant.bindings().iter().zip(0..).fold(
                quote!(false),
                |tokens, (binding, index)| {
                    let field_ty = &binding.ast().ty;
                    let field_index = Literal::usize_unsuffixed(index);

                    quote! {
                        #tokens |
                            unsafe {
                                salsa::plumbing::UpdateDispatch::<#field_ty>::maybe_update(
                                    #binding,
                                    #new_value.#field_index,
                                )
                            }
                    }
                },
            );

            quote!(
                #variant_pat => {
                    #make_new_value
                    #update_fields
                }
            )
        })
        .collect();

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let tokens = quote! {
        #[allow(clippy::all)]
        unsafe impl #impl_generics salsa::Update for #ident #ty_generics #where_clause {
            unsafe fn maybe_update(#old_pointer: *mut Self, #new_value: Self) -> bool {
                use ::salsa::plumbing::UpdateFallback as _;
                let #old_pointer = unsafe { &mut *#old_pointer };
                match #old_pointer {
                    #fields
                }
            }
        }
    };

    Ok(crate::debug::dump_tokens(&input.ident, tokens))
}
