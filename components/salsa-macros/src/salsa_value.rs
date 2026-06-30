use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream;
use syn::{Attribute, spanned::Spanned, visit::Visit, visit_mut::VisitMut};

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
    if declared_lifetime.is_some() && input.generics.params.len() > 1 {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`derive(SalsaValue)` does not support combining a lifetime with other generic parameters",
        ));
    }
    let salsa_lifetime = declared_lifetime
        .cloned()
        .unwrap_or_else(|| fresh_lifetime(&input, "'__salsa_db"));

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

    let rebound_type_params = rebound_type_params(&input.generics, &salsa_lifetime);
    let static_type = derived_type(&input, None);
    let output_type = rebind_type_params(
        &derived_type(&input, Some(&salsa_lifetime)),
        &rebound_type_params,
    );
    let mut generics = implementation_generics(&input, declared_lifetime, &salsa_lifetime);
    add_implementation_bounds(&mut generics, &salsa_lifetime, &rebound_type_params);
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let zalsa = quote::format_ident!("__salsa_plumbing");

    let field_assertions =
        checked_field_types
            .iter()
            .map(|(field_type, has_manual_retention_proof)| {
                let static_field_type = replace_self_type(field_type, &static_type);
                let static_field_type = replace_declared_lifetime(
                    &static_field_type,
                    declared_lifetime,
                    &syn::Lifetime::new("'static", field_type.span()),
                );
                let output_field_type =
                    replace_declared_lifetime(field_type, declared_lifetime, &salsa_lifetime);
                let output_field_type =
                    rebind_type_params(&output_field_type, &rebound_type_params);
                let output_field_type = replace_self_type(&output_field_type, &output_type);

                assert_salsa_value_field(
                    &salsa_lifetime,
                    &zalsa,
                    &output_field_type,
                    &static_field_type,
                    *has_manual_retention_proof,
                )
            });

    let tokens = quote! {
        #[automatically_derived]
        // SAFETY: The assertions below verify the retained representation of
        // every field.
        unsafe impl #impl_generics ::salsa::SalsaValue<#salsa_lifetime>
            for #static_type #where_clause
        {
            type Output = #output_type;
        }

        const _: () = {
            use ::salsa::plumbing as #zalsa;
            use #zalsa::{SalsaValueDispatch, SalsaValueFallback as _};

            trait SalsaValueFieldAssertions<#salsa_lifetime> {
                const ASSERT_FIELDS_ARE_SALSA_VALUES: ();
            }

            impl #impl_generics SalsaValueFieldAssertions<#salsa_lifetime>
                for #static_type #where_clause
            {
                const ASSERT_FIELDS_ARE_SALSA_VALUES: () = {
                    #(#field_assertions)*
                };
            }
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

fn rebound_type_params(
    generics: &syn::Generics,
    salsa_lifetime: &syn::Lifetime,
) -> HashMap<String, syn::Type> {
    generics
        .type_params()
        .map(|parameter| {
            let ident = &parameter.ident;
            (
                ident.to_string(),
                syn::parse_quote!(<#ident as ::salsa::SalsaValue<#salsa_lifetime>>::Output),
            )
        })
        .collect()
}

fn add_implementation_bounds(
    generics: &mut syn::Generics,
    salsa_lifetime: &syn::Lifetime,
    rebound_type_params: &HashMap<String, syn::Type>,
) {
    if rebound_type_params.is_empty() {
        return;
    }

    let original_where_predicates = generics
        .where_clause
        .iter()
        .flat_map(|where_clause| where_clause.predicates.iter().cloned())
        .collect::<Vec<_>>();
    let mut output_bounds = Vec::new();

    for param in &mut generics.params {
        let syn::GenericParam::Type(type_param) = param else {
            continue;
        };

        let output = &rebound_type_params[&type_param.ident.to_string()];
        let bounds = type_param.bounds.clone();
        if !bounds.is_empty() {
            output_bounds.push(syn::parse_quote!(#output: #bounds));
        }
        type_param
            .bounds
            .push(syn::parse_quote!(::salsa::SalsaValue<#salsa_lifetime>));
    }

    let output_where_predicates = original_where_predicates
        .iter()
        .map(|predicate| rebind_where_predicate(predicate, rebound_type_params));
    generics
        .make_where_clause()
        .predicates
        .extend(output_bounds.into_iter().chain(output_where_predicates));
}

fn rebind_where_predicate(
    predicate: &syn::WherePredicate,
    rebound_type_params: &HashMap<String, syn::Type>,
) -> syn::WherePredicate {
    let mut predicate = predicate.clone();
    ChangeTypeParams::new(rebound_type_params).visit_where_predicate_mut(&mut predicate);
    predicate
}

fn rebind_type_params(
    ty: &syn::Type,
    rebound_type_params: &HashMap<String, syn::Type>,
) -> syn::Type {
    let mut ty = ty.clone();
    ChangeTypeParams::new(rebound_type_params).visit_type_mut(&mut ty);
    ty
}

struct ChangeTypeParams<'a> {
    replacements: &'a HashMap<String, syn::Type>,
}

impl<'a> ChangeTypeParams<'a> {
    fn new(replacements: &'a HashMap<String, syn::Type>) -> Self {
        Self { replacements }
    }
}

impl VisitMut for ChangeTypeParams<'_> {
    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        let syn::Type::Path(type_path) = ty else {
            syn::visit_mut::visit_type_mut(self, ty);
            return;
        };
        if type_path.qself.is_some() || type_path.path.segments.len() != 1 {
            syn::visit_mut::visit_type_mut(self, ty);
            return;
        }

        let segment = type_path.path.segments.first().unwrap();
        let Some(replacement) = self.replacements.get(&segment.ident.to_string()) else {
            syn::visit_mut::visit_type_mut(self, ty);
            return;
        };
        if !matches!(segment.arguments, syn::PathArguments::None) {
            syn::visit_mut::visit_type_mut(self, ty);
            return;
        }

        *ty = replacement.clone();
    }

    fn visit_type_path_mut(&mut self, type_path: &mut syn::TypePath) {
        if type_path.qself.is_some() || type_path.path.segments.len() < 2 {
            syn::visit_mut::visit_type_path_mut(self, type_path);
            return;
        }

        let first = type_path.path.segments.first().unwrap();
        let Some(replacement) = self.replacements.get(&first.ident.to_string()) else {
            syn::visit_mut::visit_type_path_mut(self, type_path);
            return;
        };
        if !matches!(first.arguments, syn::PathArguments::None) {
            syn::visit_mut::visit_type_path_mut(self, type_path);
            return;
        }

        let span = first.ident.span();
        type_path.qself = Some(syn::QSelf {
            lt_token: syn::Token![<](span),
            ty: Box::new(replacement.clone()),
            position: 0,
            as_token: None,
            gt_token: syn::Token![>](span),
        });
        type_path.path.segments = type_path.path.segments.iter().skip(1).cloned().collect();
    }
}

fn implementation_generics(
    input: &syn::DeriveInput,
    declared_lifetime: Option<&syn::Lifetime>,
    salsa_lifetime: &syn::Lifetime,
) -> syn::Generics {
    let mut generics = input.generics.clone();
    if declared_lifetime.is_some() {
        generics.params.clear();
        generics.where_clause = None;
    }

    generics
        .params
        .insert(0, syn::parse_quote!(#salsa_lifetime));
    generics
}

fn derived_type(input: &syn::DeriveInput, output_lifetime: Option<&syn::Lifetime>) -> syn::Type {
    let ident = &input.ident;
    let tokens = if input.generics.lifetimes().next().is_some() {
        let lifetime = output_lifetime
            .map_or_else(|| syn::Lifetime::new("'static", ident.span()), Clone::clone);
        quote!(#ident<#lifetime>)
    } else {
        let (_, type_generics, _) = input.generics.split_for_impl();
        quote!(#ident #type_generics)
    };
    syn::parse2(tokens).expect("generated SalsaValue type should parse")
}

fn replace_self_type(ty: &syn::Type, self_type: &syn::Type) -> syn::Type {
    let mut ty = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut ChangeSelfPath::new(self_type, None), &mut ty);
    ty
}

fn replace_declared_lifetime(
    ty: &syn::Type,
    declared_lifetime: Option<&syn::Lifetime>,
    replacement: &syn::Lifetime,
) -> syn::Type {
    match declared_lifetime {
        Some(declared_lifetime) => ChangeLt::named_to(declared_lifetime, replacement).in_type(ty),
        None => ty.clone(),
    }
}

fn fresh_lifetime(input: &syn::DeriveInput, candidate: &str) -> syn::Lifetime {
    let mut lifetimes = LifetimeNames::default();
    lifetimes.visit_derive_input(input);

    let mut candidate = candidate.to_owned();
    while lifetimes.used.contains(candidate.trim_start_matches('\'')) {
        candidate.insert(1, '_');
    }
    syn::Lifetime::new(&candidate, input.ident.span())
}

#[derive(Default)]
struct LifetimeNames {
    used: HashSet<String>,
}

impl<'ast> Visit<'ast> for LifetimeNames {
    fn visit_lifetime(&mut self, lifetime: &'ast syn::Lifetime) {
        self.used.insert(lifetime.ident.to_string());
    }
}

pub(crate) fn assert_salsa_value_or_static(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
    static_ty: &syn::Type,
) -> TokenStream {
    // Prefer the direct bound when the return type names the database lifetime,
    // so rustc reports a missing `SalsaValue` impl instead of a failed `'static` fallback.
    if crate::xform::uses_lifetime(ty, db_lt) {
        return assert_tracked_output_is_salsa_value(db_lt, zalsa, ty, static_ty);
    }

    let assertion = assert_salsa_value_or_static_expr(db_lt, zalsa, ty, static_ty);
    quote! {
        fn _assert_output_is_salsa_value_or_static<#db_lt>() {
            let _ = ::core::marker::PhantomData::<&#db_lt ()>;
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
    static_ty: &syn::Type,
    has_manual_retention_proof: bool,
) -> TokenStream {
    if has_manual_retention_proof {
        quote! {}
    } else {
        assert_salsa_value_or_static_expr(db_lt, zalsa, ty, static_ty)
    }
}

fn assert_tracked_output_is_salsa_value(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
    static_ty: &syn::Type,
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
    let static_ty = ChangeLt::named_to(db_lt, &assertion_lifetime).in_type(static_ty);
    let assertion = quote_spanned! {ty.span() =>
        #zalsa::assert_salsa_value_output::<#static_ty, #ty>(
            ::core::marker::PhantomData::<&#assertion_lifetime ()>,
        );
    };
    quote! {
        fn _assert_output_is_salsa_value<#assertion_lifetime>() {
            let _ = ::core::marker::PhantomData::<&#assertion_lifetime ()>;
            #assertion
        }
        let _ = _assert_output_is_salsa_value;
    }
}

pub(crate) fn assert_salsa_value_or_static_expr(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
    static_ty: &syn::Type,
) -> TokenStream {
    if crate::xform::uses_lifetime(ty, db_lt) {
        return assert_salsa_value_expr(db_lt, zalsa, ty, static_ty);
    }

    quote_spanned! {ty.span() =>
        let _ = SalsaValueDispatch::<#db_lt, #static_ty, #ty>::assert_salsa_value;
    }
}

pub(crate) fn assert_salsa_value_expr(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    ty: &syn::Type,
    static_ty: &syn::Type,
) -> TokenStream {
    if is_db_reference(ty, db_lt) {
        return syn::Error::new_spanned(
            ty,
            "a reference tied to the database lifetime does not implement `SalsaValue`; store owned data or a Salsa handle instead",
        )
        .into_compile_error();
    }

    quote_spanned! {ty.span() =>
        #zalsa::assert_salsa_value::<#static_ty, #ty>(
            ::core::marker::PhantomData::<&#db_lt ()>,
        );
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

pub(crate) fn static_type(ty: &syn::Type, db_lifetime: &syn::Lifetime) -> syn::Type {
    replace_declared_lifetime(
        ty,
        Some(db_lifetime),
        &syn::Lifetime::new("'static", db_lifetime.span()),
    )
}
