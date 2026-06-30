use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{Attribute, spanned::Spanned, visit::Visit};

use crate::kw;

pub(crate) fn salsa_value_derive(input: syn::DeriveInput) -> syn::Result<TokenStream> {
    if let syn::Data::Union(union) = &input.data {
        return Err(syn::Error::new_spanned(
            union.union_token,
            "`derive(SalsaValue)` does not support `union`",
        ));
    }

    reject_salsa_value_attributes(&input.attrs, "type")?;

    let mut checked_field_types = Vec::new();
    let mut used_type_params = UsedTypeParams::new(&input.generics);

    match &input.data {
        syn::Data::Struct(data) => {
            collect_checked_field_types(
                &data.fields,
                &mut checked_field_types,
                &mut used_type_params,
            )?;
        }
        syn::Data::Enum(data) => {
            for variant in &data.variants {
                reject_salsa_value_attributes(&variant.attrs, "variant")?;
                collect_checked_field_types(
                    &variant.fields,
                    &mut checked_field_types,
                    &mut used_type_params,
                )?;
            }
        }
        syn::Data::Union(_) => unreachable!(),
    }

    let ident = &input.ident;
    let mut generics = input.generics.clone();
    add_salsa_value_bounds(&mut generics, &used_type_params.used);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // An associated const permits `Self` in recursive field types without
    // adding cyclic field-type bounds to the `SalsaValue` impl itself.
    let tokens = quote! {
        #[automatically_derived]
        unsafe impl #impl_generics ::salsa::SalsaValue for #ident #ty_generics #where_clause {}

        const _: () = {
            trait SalsaValueFieldAssertions {
                const ASSERT_FIELDS_ARE_SALSA_VALUES: ();
            }

            impl #impl_generics SalsaValueFieldAssertions for #ident #ty_generics #where_clause {
                const ASSERT_FIELDS_ARE_SALSA_VALUES: () = {
                    fn assert_salsa_value<T: ::salsa::SalsaValue>() {}
                    #(let _ = assert_salsa_value::<#checked_field_types>;)*
                };
            }
        };
    };

    Ok(crate::debug::dump_tokens(&input.ident, tokens))
}

fn collect_checked_field_types<'a>(
    fields: &'a syn::Fields,
    checked_field_types: &mut Vec<&'a syn::Type>,
    used_type_params: &mut UsedTypeParams,
) -> syn::Result<()> {
    for field in fields {
        if field_has_manual_retention_proof(&field.attrs)? {
            continue;
        }

        checked_field_types.push(&field.ty);
        used_type_params.visit_type(&field.ty);
    }

    Ok(())
}

fn field_has_manual_retention_proof(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut attrs = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("salsa_value"));
    let Some(attr) = attrs.next() else {
        return Ok(false);
    };

    if let Some(duplicate) = attrs.next() {
        return Err(syn::Error::new(
            duplicate.path().span(),
            "multiple `#[salsa_value]` attributes on field",
        ));
    }

    attr.parse_args::<kw::prove_safe_to_retain_manually>()?;
    Ok(true)
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

fn add_salsa_value_bounds(generics: &mut syn::Generics, used_type_params: &HashSet<String>) {
    for param in &mut generics.params {
        let syn::GenericParam::Type(type_param) = param else {
            continue;
        };

        if used_type_params.contains(&type_param.ident.to_string()) {
            type_param
                .bounds
                .push(syn::parse_quote!(::salsa::SalsaValue));
        }
    }
}

struct UsedTypeParams {
    type_params: HashSet<String>,
    used: HashSet<String>,
}

impl UsedTypeParams {
    fn new(generics: &syn::Generics) -> Self {
        let type_params = generics
            .type_params()
            .map(|type_param| type_param.ident.to_string())
            .collect();

        Self {
            type_params,
            used: HashSet::new(),
        }
    }
}

impl<'ast> Visit<'ast> for UsedTypeParams {
    fn visit_type_path(&mut self, type_path: &'ast syn::TypePath) {
        if type_path.qself.is_none() {
            if let Some(segment) = type_path.path.segments.first() {
                let ident = segment.ident.to_string();
                if self.type_params.contains(&ident) {
                    self.used.insert(ident);
                }
            }
        }

        syn::visit::visit_type_path(self, type_path);
    }
}

pub(crate) fn assert_salsa_value_or_static(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
) -> TokenStream {
    // Prefer the direct bound when the return type names the database lifetime,
    // so rustc reports a missing `SalsaValue` impl instead of a failed `'static` fallback.
    if crate::xform::uses_lifetime(ty, db_lt) {
        return assert_salsa_value(db_lt, ty);
    }

    let assertion = quote_spanned! {ty.span() =>
        let _ = SalsaValueDispatch::<#ty>::assert_salsa_value;
    };
    quote! {
        fn _assert_output_is_salsa_value_or_static<#db_lt>() {
            let _ = ::core::marker::PhantomData::<&#db_lt ()>;
            use #zalsa::{SalsaValueDispatch, SalsaValueFallback as _};
            #assertion
        }
        let _ = _assert_output_is_salsa_value_or_static;
    }
}

fn assert_salsa_value(db_lt: &syn::Lifetime, ty: &syn::Type) -> TokenStream {
    if matches!(
        ty,
        syn::Type::Reference(reference)
            if reference
                .lifetime
                .as_ref()
                .is_some_and(|lifetime| lifetime.ident == db_lt.ident)
    ) {
        return quote_spanned! {ty.span() =>
            compile_error!("a reference tied to the database lifetime does not implement `SalsaValue`; return an owned value instead");
        };
    }

    let assertion = quote_spanned! {ty.span() =>
        let _ = assert_salsa_value::<#ty>;
    };
    quote! {
        fn _assert_output_is_salsa_value<#db_lt>() {
            let _ = ::core::marker::PhantomData::<&#db_lt ()>;

            #[diagnostic::on_unimplemented(
                message = "the tracked function's return type `{Self}` does not implement `SalsaValue`",
                label = "does not implement `SalsaValue`",
                note = "consider deriving `salsa::SalsaValue` for the tracked function's return type if it is safe to retain across revisions"
            )]
            trait SalsaValue {}

            #[diagnostic::do_not_recommend]
            impl<T: ::salsa::SalsaValue> SalsaValue for T {}

            fn assert_salsa_value<T: SalsaValue>() {}
            #assertion
        }
        let _ = _assert_output_is_salsa_value;
    }
}
