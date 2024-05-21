use proc_macro2::{Literal, Span, TokenStream};

pub(crate) fn debug_with_db(input: syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    // Figure out the lifetime to use for the `dyn Db` that we will expect.
    // We allow structs to have at most one lifetime -- if a lifetime parameter is present,
    // it should be `'db`. We may want to generalize this later.

    let num_lifetimes = input.generics.lifetimes().count();
    if num_lifetimes > 1 {
        return syn::Result::Err(syn::Error::new(
            input.generics.lifetimes().nth(1).unwrap().lifetime.span(),
            "only one lifetime is supported",
        ));
    }

    let db_lt = match input.generics.lifetimes().next() {
        Some(lt) => lt.lifetime.clone(),
        None => syn::Lifetime::new("'_", Span::call_site()),
    };

    // Generate the type of database we expect. This hardcodes the convention of using `jar::Jar`.
    // That's not great and should be fixed but we'd have to add a custom attribute and I am too lazy.

    #[allow(non_snake_case)]
    let DB: syn::Type = parse_quote! {
        <crate::Jar as salsa::jar::Jar< #db_lt >>::DynDb
    };

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
                    &::salsa::debug::helper::SalsaDebug::<#field_ty, #DB>::salsa_debug(
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
                        let binding_data = binding_tokens(&binding);
                        quote!(#tokens . field(#binding_name, #binding_data))
                    },
                ),

                syn::Fields::Unnamed(_) | syn::Fields::Unit => variant.fold(
                    quote!(#fmt.debug_tuple(#variant_name)),
                    |tokens, binding| {
                        let binding_data = binding_tokens(&binding);
                        quote!(#tokens . field(#binding_data))
                    },
                ),
            };

            quote!(#fields . finish(),)
        })
        .collect();

    let tokens = structure.gen_impl(quote! {
        gen impl ::salsa::debug::DebugWithDb<#DB> for @Self {
            fn fmt(&self, #fmt: &mut std::fmt::Formatter<'_>, #db: & #DB) -> std::fmt::Result {
                match self {
                    #fields
                }
            }
        }
    });

    Ok(crate::debug::dump_tokens(&input.ident, tokens))
}
