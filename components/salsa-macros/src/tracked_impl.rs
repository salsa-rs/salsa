use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::Nothing;

use crate::{hygiene::Hygiene, tracked_fn::FnArgs};

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

struct MethodArguments<'syn> {
    self_token: &'syn syn::token::SelfValue,
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
        for member_item in &mut member_items {
            self.modify_member(&impl_item, member_item)?;
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
    ) -> syn::Result<()> {
        let syn::ImplItem::Fn(fn_item) = member_item else {
            return Ok(());
        };

        let self_ty = &impl_item.self_ty;

        let Some(tracked_attr_index) = fn_item.attrs.iter().position(|a| self.is_tracked_attr(a))
        else {
            return Ok(());
        };

        let salsa_tracked_attr = fn_item.attrs.remove(tracked_attr_index);
        let args: FnArgs = match &salsa_tracked_attr.meta {
            syn::Meta::Path(..) => Default::default(),
            _ => salsa_tracked_attr.parse_args()?,
        };

        let InnerTrait = self.hygiene.ident("InnerTrait");
        let inner_fn_name = self.hygiene.ident("inner_fn_name");

        let MethodArguments {
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

        // Construct the body of the method

        let block = parse_quote!({
            salsa::plumbing::setup_method_body! {
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
        });

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
    ) -> syn::Result<MethodArguments<'syn>> {
        let db_lt = self.extract_db_lifetime(impl_item, fn_item)?;

        let self_token = self.check_self_argument(fn_item)?;

        let (db_ident, db_ty) = self.check_db_argument(&fn_item.sig.inputs[1])?;

        let input_ids: Vec<syn::Ident> = crate::fn_util::input_ids(&self.hygiene, &fn_item.sig, 2);
        let input_tys = crate::fn_util::input_tys(&fn_item.sig, 2)?;
        let output_ty = crate::fn_util::output_ty(db_lt, &fn_item.sig)?;

        Ok(MethodArguments {
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
        if let Some(return_ref) = &args.return_ref {
            if let syn::ReturnType::Type(_, t) = &mut sig.output {
                **t = parse_quote!(& #db_lt #t)
            } else {
                return Err(syn::Error::new_spanned(
                    return_ref,
                    "return_ref attribute requires explicit return type",
                ));
            };
        }
        Ok(())
    }
}
