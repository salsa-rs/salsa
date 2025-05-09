use proc_macro2::{Literal, Span, TokenStream};
use syn::{parenthesized, parse::ParseStream, spanned::Spanned, Token};
use synstructure::BindStyle;

use crate::{hygiene::Hygiene, kw};

pub(crate) fn update_derive(input: syn::DeriveInput) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&input);

    if let syn::Data::Union(u) = &input.data {
        return Err(syn::Error::new_spanned(
            u.union_token,
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
            let err = variant
                .ast()
                .attrs
                .iter()
                .filter(|attr| attr.path().is_ident("update"))
                .map(|attr| {
                    syn::Error::new(
                        attr.path().span(),
                        "unexpected attribute `#[update]` on variant",
                    )
                })
                .reduce(|mut acc, err| {
                    acc.combine(err);
                    acc
                });
            if let Some(err) = err {
                return Err(err);
            }
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
            let mut update_fields = quote!(false);
            for (index, binding) in variant.bindings().iter().enumerate() {
                let mut attrs = binding
                    .ast()
                    .attrs
                    .iter()
                    .filter(|attr| attr.path().is_ident("update"));
                let attr = attrs.next();
                if let Some(attr) = attrs.next() {
                    return Err(syn::Error::new(
                        attr.path().span(),
                        "multiple #[update(with)] attributes on field",
                    ));
                }

                let field_ty = &binding.ast().ty;
                let field_index = Literal::usize_unsuffixed(index);

                let (maybe_update, unsafe_token) = match attr {
                    Some(attr) => {
                        attr.parse_args_with(|parser: ParseStream| {
                            let mut content;

                            let unsafe_token = parser.parse::<Token![unsafe]>()?;
                            parenthesized!(content in parser);
                            let with_token = content.parse::<kw::with>()?;
                            parenthesized!(content in content);
                            let expr = content.parse::<syn::Expr>()?;
                            Ok((
                                quote_spanned! { with_token.span() =>  ({ let maybe_update: unsafe fn(*mut #field_ty, #field_ty) -> bool = #expr; maybe_update }) },
                                unsafe_token,
                            ))
                        })?
                    }
                    None => {
                        (
                            quote!(
                                salsa::plumbing::UpdateDispatch::<#field_ty>::maybe_update
                            ),
                            Token![unsafe](Span::call_site()),
                        )
                    }
                };
                let update_field = quote! {
                    #maybe_update(
                        #binding,
                        #new_value.#field_index,
                    )
                };

                update_fields = quote! {
                    #update_fields | #unsafe_token { #update_field }
                };
            }

            Ok(quote!(
                #variant_pat => {
                    #make_new_value
                    #update_fields
                }
            ))
        })
        .collect::<syn::Result<_>>()?;

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let tokens = quote! {
        #[allow(clippy::all)]
        #[automatically_derived]
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
