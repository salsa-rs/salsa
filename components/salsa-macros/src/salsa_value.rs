use proc_macro2::TokenStream;
use syn::{Attribute, spanned::Spanned};

use crate::kw;
use crate::xform::{ChangeLt, ChangeSelfPath};

pub(crate) fn salsa_value_derive(input: syn::DeriveInput) -> syn::Result<TokenStream> {
    if let syn::Data::Union(union) = &input.data {
        return Err(syn::Error::new_spanned(
            union.union_token,
            "`derive(SalsaValue)` does not support `union`",
        ));
    }

    reject_salsa_value_attributes(&input.attrs, "type")?;

    if input.generics.type_params().next().is_some()
        || input.generics.const_params().next().is_some()
    {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`derive(SalsaValue)` does not support type or const parameters",
        ));
    }

    let mut declared_lifetimes = input.generics.lifetimes();
    let declared_lifetime = declared_lifetimes
        .next()
        .map(|parameter| &parameter.lifetime);
    if declared_lifetimes.next().is_some() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`derive(SalsaValue)` supports at most one lifetime parameter",
        ));
    }
    let mut checked_field_types = Vec::new();

    match &input.data {
        syn::Data::Struct(data) => {
            collect_checked_field_types(&data.fields, &mut checked_field_types)?;
        }
        syn::Data::Enum(data) => {
            for variant in &data.variants {
                reject_salsa_value_attributes(&variant.attrs, "variant")?;
                collect_checked_field_types(&variant.fields, &mut checked_field_types)?;
            }
        }
        syn::Data::Union(_) => unreachable!(),
    }

    let Some(salsa_lifetime) = declared_lifetime else {
        let ident = &input.ident;
        let tokens = quote! {
            #[automatically_derived]
            unsafe impl ::salsa::SalsaValue for #ident {}
        };
        return Ok(crate::debug::dump_tokens(&input.ident, tokens));
    };

    let derived_type = derived_type(&input.ident, salsa_lifetime);
    let zalsa = quote::format_ident!("__salsa_plumbing");

    let field_assertions =
        checked_field_types
            .iter()
            .map(|(field_type, has_manual_retention_proof)| {
                let field_type = replace_self_type(field_type, &derived_type);

                assert_salsa_value_field(
                    salsa_lifetime,
                    &zalsa,
                    &field_type,
                    *has_manual_retention_proof,
                )
            });

    let tokens = quote! {
        #[automatically_derived]
        // SAFETY: The assertions below verify the retained representation of
        // every field.
        unsafe impl<#salsa_lifetime> ::salsa::SalsaValue for #derived_type {}

        const _: () = {
            use ::salsa::plumbing as #zalsa;
            use #zalsa::{SalsaValueDispatch, SalsaValueFallback as _};

            fn _assert_fields_are_salsa_values<#salsa_lifetime>() {
                #(#field_assertions)*
            }
            let _ = _assert_fields_are_salsa_values;
        };
    };

    Ok(crate::debug::dump_tokens(&input.ident, tokens))
}

fn collect_checked_field_types<'a>(
    fields: &'a syn::Fields,
    checked_field_types: &mut Vec<(&'a syn::Type, bool)>,
) -> syn::Result<()> {
    for field in fields {
        checked_field_types.push((&field.ty, field_has_manual_retention_proof(field)?));
    }

    Ok(())
}

fn field_has_manual_retention_proof(field: &syn::Field) -> syn::Result<bool> {
    let mut attrs = field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("salsa_value"));
    let Some(attr) = attrs.next() else {
        return Ok(false);
    };

    if attrs.next().is_some() {
        return Err(syn::Error::new_spanned(
            field,
            "multiple `#[salsa_value]` attributes on field",
        ));
    }

    parse_manual_retention_proof(attr)
        .map_err(|error| syn::Error::new_spanned(field, error.to_string()))?;
    Ok(true)
}

pub(crate) fn parse_manual_retention_proof(attr: &Attribute) -> syn::Result<()> {
    attr.parse_args::<kw::prove_safe_to_retain_manually>()
        .map(|_| ())
}

fn reject_salsa_value_attributes(attrs: &[Attribute], target: &str) -> syn::Result<()> {
    let errors = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("salsa_value"))
        .map(|attr| {
            syn::Error::new(
                attr.path().span(),
                format!("unexpected `#[salsa_value]` attribute on {target}"),
            )
        });

    match errors.reduce(|mut combined, error| {
        combined.combine(error);
        combined
    }) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn derived_type(ident: &syn::Ident, lifetime: &syn::Lifetime) -> syn::Type {
    syn::parse_quote!(#ident<#lifetime>)
}

fn replace_self_type(ty: &syn::Type, self_type: &syn::Type) -> syn::Type {
    let mut ty = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut ChangeSelfPath::new(self_type, None), &mut ty);
    ty
}

pub(crate) fn assert_salsa_value_or_static(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
) -> TokenStream {
    // Prefer the direct bound when the return type names the database lifetime,
    // so rustc reports a missing `SalsaValue` impl instead of a failed `'static` fallback.
    if crate::xform::uses_lifetime(ty, db_lt) {
        return assert_tracked_output_is_salsa_value(db_lt, zalsa, ty);
    }

    let assertion = assert_salsa_value_or_static_expr(db_lt, zalsa, ty);
    quote! {
        fn _assert_output_is_salsa_value_or_static<#db_lt>() {
            use #zalsa::{SalsaValueDispatch, SalsaValueFallback as _};
            #assertion
        }
        let _ = _assert_output_is_salsa_value_or_static;
    }
}

pub(crate) fn assert_salsa_value_field(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
    has_manual_retention_proof: bool,
) -> TokenStream {
    if has_manual_retention_proof {
        quote! {}
    } else {
        assert_salsa_value_or_static_expr(db_lt, zalsa, ty)
    }
}

fn assert_tracked_output_is_salsa_value(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
) -> TokenStream {
    if is_db_reference(ty, db_lt) {
        return syn::Error::new_spanned(
            ty,
            "a reference tied to the database lifetime does not implement `SalsaValue`; return an owned value instead",
        )
        .into_compile_error();
    }

    let assertion_lifetime = syn::Lifetime::new(&format!("'{}", db_lt.ident), ty.span());
    let ty = ChangeLt::named_to(db_lt, &assertion_lifetime).in_type(ty);
    let assertion = quote_spanned! {ty.span() =>
        #zalsa::assert_salsa_value::<#ty>();
    };
    quote! {
        fn _assert_output_is_salsa_value<#assertion_lifetime>() {
            #assertion
        }
        let _ = _assert_output_is_salsa_value;
    }
}

fn assert_salsa_value_or_static_expr(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
) -> TokenStream {
    if crate::xform::uses_lifetime(ty, db_lt) {
        if is_db_reference(ty, db_lt) {
            return syn::Error::new_spanned(
                ty,
                "a reference tied to the database lifetime does not implement `SalsaValue`; store owned data or a Salsa struct instead",
            )
            .into_compile_error();
        }

        return quote_spanned! {ty.span() =>
            #zalsa::assert_salsa_value::<#ty>();
        };
    }

    quote_spanned! {ty.span() =>
        let _ = SalsaValueDispatch::<#ty>::assert_salsa_value;
    }
}

fn is_db_reference(ty: &syn::Type, db_lt: &syn::Lifetime) -> bool {
    matches!(
        ty,
        syn::Type::Reference(reference)
            if reference
                .lifetime
                .as_ref()
                .is_some_and(|lifetime| lifetime.ident == db_lt.ident)
    )
}
