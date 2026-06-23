use std::collections::HashSet;

use proc_macro2::{Literal, Span, TokenStream};
use syn::{Token, parenthesized, parse::ParseStream, spanned::Spanned, visit::Visit};
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
    let mut used_type_params = UsedTypeParams::new(&input.generics);
    let mut additional_bounds = Vec::new();

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
                        "multiple #[update] attributes on field",
                    ));
                }

                let field_ty = &binding.ast().ty;
                let field_index = Literal::usize_unsuffixed(index);
                let attr = attr
                    .map(|attr| attr.parse_args::<UpdateFieldArgs>())
                    .transpose()?;
                let has_escape_hatch = attr
                    .as_ref()
                    .is_some_and(UpdateFieldArgs::has_escape_hatch);
                if !has_escape_hatch {
                    used_type_params.visit_type(field_ty);
                }

                let (maybe_update, unsafe_token) = match attr {
                    Some(attr) => {
                        additional_bounds.extend(attr.bounds);
                        match attr.update_strategy {
                            Some(UpdateStrategy::With(update_with)) => {
                                let UpdateWith {
                                    unsafe_token,
                                    with_token,
                                    expr,
                                } = update_with;
                                let span = with_token.span();
                                (
                                    quote_spanned! { span =>  ({ let maybe_update: unsafe fn(*mut #field_ty, #field_ty) -> bool = #expr; maybe_update }) },
                                    unsafe_token,
                                )
                            }
                            Some(UpdateStrategy::Fallback(fallback_token)) => {
                                additional_bounds.push(syn::parse_quote!(#field_ty: 'static + ::core::cmp::PartialEq));
                                let span = fallback_token.span();
                                (
                                    quote_spanned! { span => salsa::update_fallback::<#field_ty> },
                                    Token![unsafe](Span::call_site()),
                                )
                            }
                            None => (
                                quote!(
                                    salsa::plumbing::UpdateDispatch::<#field_ty>::maybe_update
                                ),
                                Token![unsafe](Span::call_site()),
                            ),
                        }
                    }
                    None => (
                        quote!(
                            salsa::plumbing::UpdateDispatch::<#field_ty>::maybe_update
                        ),
                        Token![unsafe](Span::call_site()),
                    ),
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
    let used_type_params = used_type_params.used;
    let generics = input.generics;
    let generics = add_trait_bounds(generics, &used_type_params, additional_bounds);

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
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

fn add_trait_bounds(
    mut generics: syn::Generics,
    used_type_params: &HashSet<String>,
    additional_bounds: Vec<syn::WherePredicate>,
) -> syn::Generics {
    for param in &mut generics.params {
        let syn::GenericParam::Type(type_param) = param else {
            continue;
        };

        if used_type_params.contains(&type_param.ident.to_string()) {
            type_param.bounds.push(syn::parse_quote!(::salsa::Update));
        }
    }

    generics
        .make_where_clause()
        .predicates
        .extend(additional_bounds);

    generics
}

struct UpdateFieldArgs {
    bounds: Vec<syn::WherePredicate>,
    update_strategy: Option<UpdateStrategy>,
}

impl UpdateFieldArgs {
    fn has_escape_hatch(&self) -> bool {
        self.update_strategy.is_some()
    }
}

impl syn::parse::Parse for UpdateFieldArgs {
    fn parse(parser: ParseStream<'_>) -> syn::Result<Self> {
        let mut bounds = Vec::new();
        let mut update_strategy: Option<UpdateStrategy> = None;

        while !parser.is_empty() {
            if parser.peek(kw::bounds) {
                let content;

                parser.parse::<kw::bounds>()?;
                parenthesized!(content in parser);
                bounds.extend(content.parse_terminated(syn::WherePredicate::parse, Token![,])?);
            } else if parser.peek(Token![unsafe]) {
                if let Some(update_strategy) = update_strategy.as_ref() {
                    return Err(syn::Error::new(
                        update_strategy.span(),
                        "multiple update strategies in `#[update]` attribute",
                    ));
                }

                let mut content;

                let unsafe_token = parser.parse::<Token![unsafe]>()?;
                parenthesized!(content in parser);
                let with_token = content.parse::<kw::with>()?;
                parenthesized!(content in content);
                let expr = content.parse::<syn::Expr>()?;
                if !content.is_empty() {
                    return Err(content.error("unexpected tokens in update function"));
                }
                update_strategy = Some(UpdateStrategy::With(UpdateWith {
                    unsafe_token,
                    with_token,
                    expr,
                }));
            } else if parser.peek(kw::fallback) {
                if let Some(update_strategy) = update_strategy.as_ref() {
                    return Err(syn::Error::new(
                        update_strategy.span(),
                        "multiple update strategies in `#[update]` attribute",
                    ));
                }

                update_strategy = Some(UpdateStrategy::Fallback(parser.parse()?));
            } else if parser.peek(kw::with) {
                return Err(parser.error("expected `unsafe`"));
            } else {
                return Err(parser.error("expected `bounds`, `fallback`, or `unsafe`"));
            }

            if !parser.is_empty() {
                parser.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            bounds,
            update_strategy,
        })
    }
}

struct UpdateWith {
    unsafe_token: Token![unsafe],
    with_token: kw::with,
    expr: syn::Expr,
}

enum UpdateStrategy {
    With(UpdateWith),
    Fallback(kw::fallback),
}

impl UpdateStrategy {
    fn span(&self) -> Span {
        match self {
            Self::With(UpdateWith { unsafe_token, .. }) => unsafe_token.span,
            Self::Fallback(fallback_token) => fallback_token.span(),
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
    fn visit_type_path(&mut self, i: &'ast syn::TypePath) {
        if i.qself.is_none() {
            if let Some(segment) = i.path.segments.first() {
                let ident = segment.ident.to_string();
                if self.type_params.contains(&ident) {
                    self.used.insert(ident);
                }
            }
        }

        syn::visit::visit_type_path(self, i);
    }
}

pub(crate) fn assert_update(
    db_lt: &syn::Lifetime,
    zalsa: &syn::Ident,
    types: impl IntoIterator<Item = syn::Type>,
) -> TokenStream {
    // The path expression is responsible for emitting the primary span in the diagnostic we
    // want, so by uniformly using `ty.span()` we ensure that the diagnostic is emitted
    // at the type in the original input.
    // See the tests/compile-fail/tracked_fn_return_ref.rs test
    let maybe_update_paths = types.into_iter().map(|ty| {
        quote_spanned! {ty.span() =>
            UpdateDispatch::<#ty>::maybe_update
        }
    });
    quote! {
        #[allow(clippy::all, warnings)]
        fn _assert_return_type_is_update<#db_lt>()  {
            use #zalsa::{UpdateFallback, UpdateDispatch};
            #( #maybe_update_paths; )*
        }
    }
}
