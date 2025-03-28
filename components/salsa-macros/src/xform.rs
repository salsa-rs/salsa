use std::collections::HashSet;

use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;

pub(crate) struct ChangeLt<'a> {
    from: Option<&'a str>,
    to: String,
}

impl ChangeLt<'_> {
    pub fn elided_to(db_lt: &syn::Lifetime) -> Self {
        ChangeLt {
            from: Some("_"),
            to: db_lt.ident.to_string(),
        }
    }

    pub fn in_type(mut self, ty: &syn::Type) -> syn::Type {
        let mut ty = ty.clone();
        self.visit_type_mut(&mut ty);
        ty
    }
}

impl syn::visit_mut::VisitMut for ChangeLt<'_> {
    fn visit_lifetime_mut(&mut self, i: &mut syn::Lifetime) {
        if self.from.map(|f| i.ident == f).unwrap_or(true) {
            i.ident = syn::Ident::new(&self.to, i.ident.span());
        }
    }
}

pub(crate) struct ChangeSelfPath<'a> {
    self_ty: &'a syn::Type,
    trait_: Option<(&'a syn::Path, &'a HashSet<syn::Ident>)>,
}

impl ChangeSelfPath<'_> {
    pub fn new<'a>(
        self_ty: &'a syn::Type,
        trait_: Option<(&'a syn::Path, &'a HashSet<syn::Ident>)>,
    ) -> ChangeSelfPath<'a> {
        ChangeSelfPath { self_ty, trait_ }
    }
}

impl syn::visit_mut::VisitMut for ChangeSelfPath<'_> {
    fn visit_type_mut(&mut self, i: &mut syn::Type) {
        if let syn::Type::Path(syn::TypePath { qself: None, path }) = i {
            if path.segments.len() == 1 && path.segments.first().is_some_and(|s| s.ident == "Self")
            {
                let span = path.segments.first().unwrap().span();
                *i = respan(self.self_ty, span);
            }
        }
        syn::visit_mut::visit_type_mut(self, i);
    }

    fn visit_type_path_mut(&mut self, i: &mut syn::TypePath) {
        // `<Self as ..>` cases are handled in `visit_type_mut`
        if i.qself.is_some() {
            syn::visit_mut::visit_type_path_mut(self, i);
            return;
        }

        // A single path `Self` case is handled in `visit_type_mut`
        if i.path.segments.first().is_some_and(|s| s.ident == "Self") && i.path.segments.len() > 1 {
            let span = i.path.segments.first().unwrap().span();
            let ty = Box::new(respan::<syn::Type>(self.self_ty, span));
            let lt_token = syn::Token![<](span);
            let gt_token = syn::Token![>](span);
            match self.trait_ {
                // If the next segment's ident is a trait member, replace `Self::` with
                // `<ActualTy as Trait>::`
                Some((trait_, member_idents))
                    if member_idents.contains(&i.path.segments.iter().nth(1).unwrap().ident) =>
                {
                    let qself = syn::QSelf {
                        lt_token,
                        ty,
                        position: trait_.segments.len(),
                        as_token: Some(syn::Token![as](span)),
                        gt_token,
                    };
                    i.qself = Some(qself);
                    i.path.segments = Punctuated::from_iter(
                        trait_
                            .segments
                            .iter()
                            .chain(i.path.segments.iter().skip(1))
                            .cloned(),
                    );
                }
                // Replace `Self::` with `<ActualTy>::` otherwise
                _ => {
                    let qself = syn::QSelf {
                        lt_token,
                        ty,
                        position: 0,
                        as_token: None,
                        gt_token,
                    };
                    i.qself = Some(qself);
                    i.path.segments =
                        Punctuated::from_iter(i.path.segments.iter().skip(1).cloned());
                }
            }
        }

        syn::visit_mut::visit_type_path_mut(self, i);
    }
}

fn respan<T>(t: &T, span: proc_macro2::Span) -> T
where
    T: ToTokens + Spanned + syn::parse::Parse,
{
    let tokens = t.to_token_stream();
    let respanned = respan_tokenstream(tokens, span);
    syn::parse2(respanned).unwrap()
}

fn respan_tokenstream(
    stream: proc_macro2::TokenStream,
    span: proc_macro2::Span,
) -> proc_macro2::TokenStream {
    stream
        .into_iter()
        .map(|token| respan_token(token, span))
        .collect()
}

fn respan_token(
    mut token: proc_macro2::TokenTree,
    span: proc_macro2::Span,
) -> proc_macro2::TokenTree {
    if let proc_macro2::TokenTree::Group(g) = &mut token {
        *g = proc_macro2::Group::new(g.delimiter(), respan_tokenstream(g.stream(), span));
    }
    token.set_span(span);
    token
}
