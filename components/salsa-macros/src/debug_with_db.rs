use proc_macro2::{Literal, Span, TokenStream};

pub(crate) fn debug_with_db(input: syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let num_lifetimes = input.generics.lifetimes().count();
    if num_lifetimes > 1 {
        return syn::Result::Err(syn::Error::new(
            input.generics.lifetimes().nth(1).unwrap().lifetime.span(),
            "only one lifetime is supported",
        ));
    }

    let structure: synstructure::Structure = synstructure::Structure::new(&input);

    let fmt = syn::Ident::new("fmt", Span::call_site());
    let db = syn::Ident::new("db", Span::call_site());

    // Generic the match arm for each variant.
    let fields: TokenStream = structure
        .variants()
        .iter()
        .map(|variant| {
            let variant_name = &variant.ast().ident;
            let variant_name = Literal::string(&variant_name.to_string());

            // Closure: given a binding, generate a call to the `salsa_debug` helper to either
            // print its "debug with db" value or just use `std::fmt::Debug`. This is a nice hack that
            // lets us use `debug_with_db` when available; it won't work great for generic types unless we add
            // `DebugWithDb` bounds though.
            let binding_tokens = |binding: &synstructure::BindingInfo| {
                let field_ty = &binding.ast().ty;
                quote!(
                    &::salsa::debug::helper::SalsaDebug::<#field_ty, DB>::salsa_debug(
                        #binding,
                        #db,
                    )
                )
            };

            // Create something like `fmt.debug_struct(...).field().field().finish()`
            // for each struct field; the values to be debugged are created by
            // the `binding_tokens` closure above.
            let fields = match variant.ast().fields {
                syn::Fields::Named(_) => variant.fold(
                    quote!(#fmt.debug_struct(#variant_name)),
                    |tokens, binding| {
                        let binding_name =
                            Literal::string(&binding.ast().ident.as_ref().unwrap().to_string());
                        let binding_data = binding_tokens(binding);
                        quote!(#tokens . field(#binding_name, #binding_data))
                    },
                ),

                syn::Fields::Unnamed(_) | syn::Fields::Unit => variant.fold(
                    quote!(#fmt.debug_tuple(#variant_name)),
                    |tokens, binding| {
                        let binding_data = binding_tokens(binding);
                        quote!(#tokens . field(#binding_data))
                    },
                ),
            };

            quote!(#fields . finish(),)
        })
        .collect();

    let tokens = structure.gen_impl(quote! {
        gen impl<DB: ?Sized + crate::__salsa_crate_Db> ::salsa::debug::DebugWithDb<DB> for @Self {
            fn fmt(&self, #fmt: &mut std::fmt::Formatter<'_>, #db: &DB) -> std::fmt::Result {
                use ::salsa::debug::helper::Fallback as _;
                match self {
                    #fields
                }
            }
        }
    });

    Ok(crate::debug::dump_tokens(&input.ident, tokens))
}
