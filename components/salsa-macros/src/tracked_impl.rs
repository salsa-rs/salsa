use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::Nothing;
use syn::visit_mut::VisitMut;

use crate::hygiene::Hygiene;
use crate::tracked_fn::FnArgs;
use crate::xform::ChangeSelfPath;

pub(crate) fn tracked_impl(
    args: proc_macro::TokenStream,
    item: syn::ItemImpl,
) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&item);
    let _: Nothing = syn::parse(args)?;
    let m = Macro { hygiene };
    let generated = m.try_generate(item)?;
    Ok(generated)
}

struct Macro {
    hygiene: Hygiene,
}

struct AssociatedFunctionArguments<'syn> {
    self_token: Option<&'syn syn::token::SelfValue>,
    db_ty: &'syn syn::Type,
    db_ident: &'syn syn::Ident,
    db_lt: Option<&'syn syn::Lifetime>,
    input_ids: Vec<syn::Ident>,
    input_tys: Vec<&'syn syn::Type>,
    output_ty: syn::Type,
}

impl Macro {
    fn try_generate(&self, mut impl_item: syn::ItemImpl) -> syn::Result<TokenStream> {
        let mut member_items = std::mem::take(&mut impl_item.items);
        let member_idents: HashSet<_> = member_items
            .iter()
            .filter_map(|item| match item {
                syn::ImplItem::Const(it) => Some(it.ident.clone()),
                syn::ImplItem::Fn(it) => Some(it.sig.ident.clone()),
                syn::ImplItem::Type(it) => Some(it.ident.clone()),
                syn::ImplItem::Macro(_) => None,
                syn::ImplItem::Verbatim(_) => None,
                _ => None,
            })
            .collect();
        for member_item in &mut member_items {
            self.modify_member(&impl_item, member_item, &member_idents)?;
        }
        impl_item.items = member_items;
        Ok(crate::debug::dump_tokens(
            format!("impl {:?}", impl_item.self_ty),
            impl_item.into_token_stream(),
        ))
    }

    #[allow(non_snake_case)]
    fn modify_member(
        &self,
        impl_item: &syn::ItemImpl,
        member_item: &mut syn::ImplItem,
        member_idents: &HashSet<syn::Ident>,
    ) -> syn::Result<()> {
        let syn::ImplItem::Fn(fn_item) = member_item else {
            return Ok(());
        };

        let self_ty = &*impl_item.self_ty;

        let Some(tracked_attr_index) = fn_item.attrs.iter().position(|a| self.is_tracked_attr(a))
        else {
            return Ok(());
        };

        let trait_ = match &impl_item.trait_ {
            Some((None, path, _)) => Some((path, member_idents)),
            _ => None,
        };
        let mut change = ChangeSelfPath::new(self_ty, trait_);
        change.visit_impl_item_fn_mut(fn_item);

        let mut salsa_tracked_attr = fn_item.attrs.remove(tracked_attr_index);
        let mut args: FnArgs = match &salsa_tracked_attr.meta {
            syn::Meta::Path(..) => Default::default(),
            _ => salsa_tracked_attr.parse_args()?,
        };
        if args.self_ty.is_none() {
            // If the user did not specify a self_ty, we use the impl's self_ty
            args.self_ty = Some(self_ty.clone());
        }
        salsa_tracked_attr.meta = syn::Meta::List(syn::MetaList {
            path: salsa_tracked_attr.path().clone(),
            delimiter: syn::MacroDelimiter::Paren(syn::token::Paren::default()),
            tokens: quote! {#args},
        });

        let InnerTrait = self.hygiene.ident("InnerTrait");
        let inner_fn_name = self.hygiene.ident(fn_item.sig.ident.to_string());

        let AssociatedFunctionArguments {
            self_token,
            db_ty,
            db_ident,
            db_lt,
            input_ids,
            input_tys,
            output_ty,
        } = self.validity_check(impl_item, fn_item)?;

        let mut inner_fn = fn_item.clone();
        inner_fn.vis = syn::Visibility::Inherited;
        inner_fn.sig.ident = inner_fn_name.clone();

        // Construct the body of the method or associated function

        let block = if let Some(self_token) = self_token {
            parse_quote!({
                salsa::plumbing::setup_tracked_method_body! {
                    salsa_tracked_attr: #salsa_tracked_attr,
                    self: #self_token,
                    self_ty: #self_ty,
                    db_lt: #db_lt,
                    db: #db_ident,
                    db_ty: (#db_ty),
                    input_ids: [#(#input_ids),*],
                    input_tys: [#(#input_tys),*],
                    output_ty: #output_ty,
                    inner_fn_name: #inner_fn_name,
                    inner_fn: #inner_fn,

                    // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
                    // We have the procedural macro generate names for those items that are
                    // not used elsewhere in the user's code.
                    unused_names: [
                        #InnerTrait,
                    ]
                }
            })
        } else {
            parse_quote!({
                salsa::plumbing::setup_tracked_assoc_fn_body! {
                    salsa_tracked_attr: #salsa_tracked_attr,
                    self_ty: #self_ty,
                    db_lt: #db_lt,
                    db: #db_ident,
                    db_ty: (#db_ty),
                    input_ids: [#(#input_ids),*],
                    input_tys: [#(#input_tys),*],
                    output_ty: #output_ty,
                    inner_fn_name: #inner_fn_name,
                    inner_fn: #inner_fn,

                    // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
                    // We have the procedural macro generate names for those items that are
                    // not used elsewhere in the user's code.
                    unused_names: [
                        #InnerTrait,
                    ]
                }
            })
        };

        // Update the method that will actually appear in the impl to have the new body
        // and its true return type
        let db_lt = db_lt.cloned();
        self.update_return_type(&mut fn_item.sig, &args, &db_lt)?;
        fn_item.block = block;

        Ok(())
    }

    fn validity_check<'syn>(
        &self,
        impl_item: &'syn syn::ItemImpl,
        fn_item: &'syn syn::ImplItemFn,
    ) -> syn::Result<AssociatedFunctionArguments<'syn>> {
        let db_lt = self.extract_db_lifetime(impl_item, fn_item)?;

        let is_method = matches!(&fn_item.sig.inputs[0], syn::FnArg::Receiver(_));

        let (self_token, db_input_index, skipped_inputs) = if is_method {
            (Some(self.check_self_argument(fn_item)?), 1, 2)
        } else {
            (None, 0, 1)
        };

        let (db_ident, db_ty) = self.check_db_argument(&fn_item.sig.inputs[db_input_index])?;

        let input_ids: Vec<syn::Ident> =
            crate::fn_util::input_ids(&self.hygiene, &fn_item.sig, skipped_inputs);
        let input_tys = crate::fn_util::input_tys(&fn_item.sig, skipped_inputs)?;
        let output_ty = crate::fn_util::output_ty(db_lt, &fn_item.sig)?;

        Ok(AssociatedFunctionArguments {
            self_token,
            db_ident,
            db_lt,
            db_ty,
            input_ids,
            input_tys,
            output_ty,
        })
    }

    fn is_tracked_attr(&self, attr: &syn::Attribute) -> bool {
        if attr.path().segments.len() != 2 {
            return false;
        }

        let seg0 = &attr.path().segments[0];
        let seg1 = &attr.path().segments[1];

        seg0.ident == "salsa"
            && seg1.ident == "tracked"
            && seg0.arguments.is_empty()
            && seg1.arguments.is_empty()
    }

    fn extract_db_lifetime<'syn>(
        &self,
        impl_item: &'syn syn::ItemImpl,
        fn_item: &'syn syn::ImplItemFn,
    ) -> syn::Result<Option<&'syn syn::Lifetime>> {
        // Either the impl XOR the fn can have generics, and it must be at most a lifetime
        let mut db_lt = None;
        for param in impl_item
            .generics
            .params
            .iter()
            .chain(fn_item.sig.generics.params.iter())
        {
            match param {
                syn::GenericParam::Lifetime(lt) => {
                    if db_lt.is_none() {
                        if let Some(bound) = lt.bounds.iter().next() {
                            return Err(syn::Error::new_spanned(
                                bound,
                                "lifetime parameters on tracked methods must not have bounds",
                            ));
                        }

                        db_lt = Some(&lt.lifetime);
                    } else {
                        return Err(syn::Error::new_spanned(
                            param,
                            "tracked method already has a lifetime parameter in scope",
                        ));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        param,
                        "tracked methods cannot have non-lifetime generic parameters",
                    ));
                }
            }
        }

        Ok(db_lt)
    }

    fn check_self_argument<'syn>(
        &self,
        fn_item: &'syn syn::ImplItemFn,
    ) -> syn::Result<&'syn syn::token::SelfValue> {
        if fn_item.sig.inputs.is_empty() {
            return Err(syn::Error::new_spanned(
                &fn_item.sig.ident,
                "tracked methods must have arguments",
            ));
        }

        let syn::FnArg::Receiver(syn::Receiver {
            attrs: _,
            self_token,
            reference,
            mutability: _,
            colon_token,
            ty: _,
        }) = &fn_item.sig.inputs[0]
        else {
            return Err(syn::Error::new_spanned(
                &fn_item.sig.inputs[0],
                "tracked methods must take a `self` argument",
            ));
        };

        if let Some(colon_token) = colon_token {
            return Err(syn::Error::new_spanned(
                colon_token,
                "tracked method's `self` argument must not have an explicit type",
            ));
        }

        if let Some((and_token, _)) = reference {
            return Err(syn::Error::new_spanned(
                and_token,
                "tracked methods's first argument must be declared as `self`, not `&self` or `&mut self`",
            ));
        }

        Ok(self_token)
    }

    fn check_db_argument<'syn>(
        &self,
        input: &'syn syn::FnArg,
    ) -> syn::Result<(&'syn syn::Ident, &'syn syn::Type)> {
        let syn::FnArg::Typed(typed) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "tracked methods must take a database parameter",
            ));
        };

        let syn::Pat::Ident(db_pat_ident) = &*typed.pat else {
            return Err(syn::Error::new_spanned(
                &typed.pat,
                "database parameter must have a simple name",
            ));
        };

        let db_ident = &db_pat_ident.ident;
        let db_ty = &*typed.ty;

        Ok((db_ident, db_ty))
    }

    fn update_return_type(
        &self,
        sig: &mut syn::Signature,
        args: &FnArgs,
        db_lt: &Option<syn::Lifetime>,
    ) -> syn::Result<()> {
        if let Some(returns) = &args.returns {
            if let syn::ReturnType::Type(_, t) = &mut sig.output {
                if returns == "copy" || returns == "clone" {
                    // leave as is
                } else if returns == "ref" {
                    **t = parse_quote!(& #db_lt #t)
                } else if returns == "deref" {
                    **t = parse_quote!(& #db_lt <#t as ::core::ops::Deref>::Target)
                } else if returns == "as_ref" {
                    **t = parse_quote!(<#t as ::salsa::SalsaAsRef>::AsRef<#db_lt>)
                } else if returns == "as_deref" {
                    **t = parse_quote!(<#t as ::salsa::SalsaAsDeref>::AsDeref<#db_lt>)
                } else {
                    return Err(syn::Error::new_spanned(
                        returns,
                        format!("Unknown returns mode `{returns}`"),
                    ));
                }
            } else {
                return Err(syn::Error::new_spanned(
                    returns,
                    "returns attribute requires explicit return type",
                ));
            };
        }
        Ok(())
    }
}
