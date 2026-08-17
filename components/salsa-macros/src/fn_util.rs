use proc_macro2::{TokenStream, TokenTree};
use syn::punctuated::Punctuated;

use crate::hygiene::Hygiene;
use crate::xform::ChangeLt;

/// Returns whether to keep an attribute on the generated inner function.
///
/// Keep `doc`, `must_use`, and `deprecated` on the public query only.
/// Rust does not allow `must_use` or `deprecated` on methods in trait impls.
/// Filter attributes inside `cfg_attr` too.
pub fn retain_body_attr(attr: &mut syn::Attribute) -> bool {
    retain_body_meta(&mut attr.meta)
}

/// Returns a vector of ids representing the function arguments.
/// Prefers to reuse the names given by the user, if possible.
pub fn input_ids(hygiene: &Hygiene, sig: &syn::Signature, skip: usize) -> Vec<syn::Ident> {
    sig.inputs
        .iter()
        .skip(skip)
        .zip(0..)
        .map(|(input, index)| {
            if let syn::FnArg::Typed(typed) = input {
                if let syn::Pat::Ident(ident) = &*typed.pat {
                    return ident.ident.clone();
                }
            }

            hygiene.ident(format!("input{index}"))
        })
        .collect()
}

pub fn input_tys(sig: &syn::Signature, skip: usize) -> syn::Result<Vec<&syn::Type>> {
    sig.inputs
        .iter()
        .skip(skip)
        .map(|input| {
            if let syn::FnArg::Typed(typed) = input {
                Ok(&*typed.ty)
            } else {
                Err(syn::Error::new_spanned(input, "unexpected receiver"))
            }
        })
        .collect()
}

pub fn output_ty(db_lt: Option<&syn::Lifetime>, sig: &syn::Signature) -> syn::Result<syn::Type> {
    match &sig.output {
        syn::ReturnType::Default => Ok(parse_quote!(())),
        syn::ReturnType::Type(_, ty) => match db_lt {
            Some(db_lt) => Ok(ChangeLt::elided_to(db_lt).in_type(ty)),
            None => Ok(syn::Type::clone(ty)),
        },
    }
}

fn retain_body_meta(meta: &mut syn::Meta) -> bool {
    let path = meta.path();
    if path.is_ident("must_use") || path.is_ident("deprecated") || path.is_ident("doc") {
        return false;
    }
    if !path.is_ident("cfg_attr") {
        return true;
    }

    let syn::Meta::List(list) = meta else {
        return true;
    };
    let Ok((predicate, attrs)) = list.parse_args_with(|input: syn::parse::ParseStream<'_>| {
        // Copy the condition without parsing it. rustc evaluates it.
        let mut predicate = TokenStream::new();
        while !input.is_empty() && !input.peek(syn::Token![,]) {
            predicate.extend([input.parse::<TokenTree>()?]);
        }
        input.parse::<syn::Token![,]>()?;
        let attrs = Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        Ok((predicate, attrs))
    }) else {
        // Keep invalid attributes so rustc can report the error.
        return true;
    };

    let mut attrs: Vec<_> = attrs.into_iter().collect();
    attrs.retain_mut(retain_body_meta);
    if attrs.is_empty() {
        return false;
    }
    list.tokens = quote!(#predicate, #(#attrs),*);
    true
}

#[cfg(test)]
mod tests {
    use super::retain_body_attr;

    #[test]
    fn retains_non_api_attributes() {
        let mut original: syn::ItemFn = parse_quote! {
            #[doc = "Public documentation"]
            #[must_use = "Use the result"]
            #[deprecated]
            #[allow(unused_variables)]
            #[expect(unused_mut)]
            #[inline]
            #[other::must_use]
            fn query() {}
        };
        let expected: syn::ItemFn = parse_quote! {
            #[allow(unused_variables)]
            #[expect(unused_mut)]
            #[inline]
            #[other::must_use]
            fn query() {}
        };
        original.attrs.retain_mut(retain_body_attr);
        assert_eq!(original.attrs, expected.attrs);
    }

    #[test]
    fn filters_nested_conditional_attributes() {
        let mut original: syn::ItemFn = parse_quote! {
            #[cfg_attr(all(), must_use, deprecated)]
            #[cfg_attr(true, doc = "Public", cfg_attr(any(), must_use))]
            #[cfg_attr(all(), must_use, cfg_attr(true, deprecated, inline), allow(unused_variables))]
            fn query() {}
        };
        let expected: syn::ItemFn = parse_quote! {
            #[cfg_attr(all(), cfg_attr(true, inline), allow(unused_variables))]
            fn query() {}
        };
        original.attrs.retain_mut(retain_body_attr);
        assert_eq!(original.attrs, expected.attrs);

        let mut malformed: syn::ItemFn = parse_quote! {
            #[cfg_attr(all(), must_use, doc =)]
            fn query() {}
        };
        let original_attrs = malformed.attrs.clone();
        malformed.attrs.retain_mut(retain_body_attr);
        assert_eq!(malformed.attrs, original_attrs);
    }
}
