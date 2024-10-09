use proc_macro2::{Literal, Span, TokenStream};
use quote::ToTokens;
use syn::{spanned::Spanned, ItemFn};

use crate::{db_lifetime, fn_util, hygiene::Hygiene, options::Options};

// Source:
//
// #[salsa::db]
// pub struct Database {
//    storage: salsa::Storage<Self>,
// }

pub(crate) fn tracked_fn(args: proc_macro::TokenStream, item: ItemFn) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&item);
    let args: FnArgs = syn::parse(args)?;
    let db_macro = Macro { hygiene, args };
    db_macro.try_fn(item)
}

pub type FnArgs = Options<TrackedFn>;

pub struct TrackedFn;

impl crate::options::AllowedOptions for TrackedFn {
    const RETURN_REF: bool = true;

    const SPECIFY: bool = true;

    const NO_EQ: bool = true;

    const NO_DEBUG: bool = false;

    const NO_CLONE: bool = false;

    const SINGLETON: bool = false;

    const DATA: bool = false;

    const DB: bool = false;

    const CYCLE_FN: bool = true;

    const CYCLE_INITIAL: bool = true;

    const LRU: bool = true;

    const CONSTRUCTOR_NAME: bool = false;
}

struct Macro {
    hygiene: Hygiene,
    args: FnArgs,
}

struct ValidFn<'item> {
    db_ident: &'item syn::Ident,
    db_path: &'item syn::Path,
}

#[allow(non_snake_case)]
impl Macro {
    fn try_fn(&self, item: syn::ItemFn) -> syn::Result<TokenStream> {
        let ValidFn { db_ident, db_path } = self.validity_check(&item)?;

        let attrs = &item.attrs;
        let fn_name = &item.sig.ident;
        let vis = &item.vis;
        let db_lt = db_lifetime::db_lifetime(&item.sig.generics);
        let input_ids = self.input_ids(&item);
        let input_tys = self.input_tys(&item)?;
        let output_ty = self.output_ty(&db_lt, &item)?;
        let (cycle_recovery_fn, cycle_recovery_initial, cycle_recovery_strategy) =
            self.cycle_recovery()?;
        let is_specifiable = self.args.specify.is_some();
        let no_eq = self.args.no_eq.is_some();

        let mut inner_fn = item.clone();
        inner_fn.vis = syn::Visibility::Inherited;
        inner_fn.sig.ident = self.hygiene.ident("inner");

        let zalsa = self.hygiene.ident("zalsa");
        let Configuration = self.hygiene.ident("Configuration");
        let InternedData = self.hygiene.ident("InternedData");
        let FN_CACHE = self.hygiene.ident("FN_CACHE");
        let INTERN_CACHE = self.hygiene.ident("INTERN_CACHE");
        let inner = &inner_fn.sig.ident;

        let function_type = function_type(&item);

        if is_specifiable {
            match function_type {
                FunctionType::Constant | FunctionType::RequiresInterning => {
                    return Err(syn::Error::new_spanned(
                        self.args.specify.as_ref().unwrap(),
                        "only functions with a single salsa struct as their input can be specified",
                    ))
                }
                FunctionType::SalsaStruct => {}
            }
        }

        if let (Some(_), Some(token)) = (&self.args.lru, &self.args.specify) {
            return Err(syn::Error::new_spanned(
                token,
                "the `specify` and `lru` options cannot be used together",
            ));
        }

        let needs_interner = match function_type {
            FunctionType::Constant | FunctionType::RequiresInterning => true,
            FunctionType::SalsaStruct => false,
        };

        let lru = Literal::usize_unsuffixed(self.args.lru.unwrap_or(0));

        let return_ref: bool = self.args.return_ref.is_some();

        Ok(crate::debug::dump_tokens(
            fn_name,
            quote![salsa::plumbing::setup_tracked_fn! {
                attrs: [#(#attrs),*],
                vis: #vis,
                fn_name: #fn_name,
                db_lt: #db_lt,
                Db: #db_path,
                db: #db_ident,
                input_ids: [#(#input_ids),*],
                input_tys: [#(#input_tys),*],
                output_ty: #output_ty,
                inner_fn: #inner_fn,
                cycle_recovery_fn: #cycle_recovery_fn,
                cycle_recovery_initial: #cycle_recovery_initial,
                cycle_recovery_strategy: #cycle_recovery_strategy,
                is_specifiable: #is_specifiable,
                no_eq: #no_eq,
                needs_interner: #needs_interner,
                lru: #lru,
                return_ref: #return_ref,
                unused_names: [
                    #zalsa,
                    #Configuration,
                    #InternedData,
                    #FN_CACHE,
                    #INTERN_CACHE,
                    #inner,
                ]
            }],
        ))
    }

    fn validity_check<'item>(&self, item: &'item syn::ItemFn) -> syn::Result<ValidFn<'item>> {
        db_lifetime::require_optional_db_lifetime(&item.sig.generics)?;

        if item.sig.inputs.is_empty() {
            return Err(syn::Error::new_spanned(
                &item.sig.ident,
                "tracked functions must have at least a database argument",
            ));
        }

        let (db_ident, db_path) =
            check_db_argument(&item.sig.inputs[0], item.sig.generics.lifetimes().next())?;

        Ok(ValidFn { db_ident, db_path })
    }
    fn cycle_recovery(&self) -> syn::Result<(TokenStream, TokenStream, TokenStream)> {
        match (&self.args.cycle_fn, &self.args.cycle_initial) {
            (Some(cycle_fn), Some(cycle_initial)) => Ok((
                quote!((#cycle_fn)),
                quote!((#cycle_initial)),
                quote!(Recover),
            )),
            (None, None) => Ok((
                quote!((salsa::plumbing::unexpected_cycle_recovery!)),
                quote!((salsa::plumbing::unexpected_cycle_initial!)),
                quote!(Panic),
            )),
            (Some(_), None) => Err(syn::Error::new_spanned(
                self.args.cycle_fn.as_ref().unwrap(),
                "must provide `cycle_initial` along with `cycle_fn`",
            )),
            (None, Some(_)) => Err(syn::Error::new_spanned(
                self.args.cycle_initial.as_ref().unwrap(),
                "must provide `cycle_fn` along with `cycle_initial`",
            )),
        }
    }

    fn input_ids(&self, item: &ItemFn) -> Vec<syn::Ident> {
        fn_util::input_ids(&self.hygiene, &item.sig, 1)
    }

    fn input_tys<'syn>(&self, item: &'syn ItemFn) -> syn::Result<Vec<&'syn syn::Type>> {
        fn_util::input_tys(&item.sig, 1)
    }

    fn output_ty(&self, db_lt: &syn::Lifetime, item: &syn::ItemFn) -> syn::Result<syn::Type> {
        fn_util::output_ty(Some(db_lt), &item.sig)
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum FunctionType {
    Constant,
    SalsaStruct,
    RequiresInterning,
}

fn function_type(item_fn: &syn::ItemFn) -> FunctionType {
    match item_fn.sig.inputs.len() {
        0 => unreachable!(
            "functions have been checked to have at least a database argument by this point"
        ),
        1 => FunctionType::Constant,
        2 => FunctionType::SalsaStruct,
        _ => FunctionType::RequiresInterning,
    }
}

pub fn check_db_argument<'arg>(
    fn_arg: &'arg syn::FnArg,
    explicit_lt: Option<&'arg syn::LifetimeParam>,
) -> syn::Result<(&'arg syn::Ident, &'arg syn::Path)> {
    match fn_arg {
        syn::FnArg::Receiver(_) => {
            // If we see `&self` where a database was expected, that indicates
            // that `#[tracked]` was applied to a method.
            Err(syn::Error::new_spanned(
                fn_arg,
                "#[salsa::tracked] must also be applied to the impl block for tracked methods",
            ))
        }
        syn::FnArg::Typed(typed) => {
            let syn::Pat::Ident(db_pat_ident) = &*typed.pat else {
                return Err(syn::Error::new_spanned(
                    &typed.pat,
                    "database parameter must have a simple name",
                ));
            };

            let syn::PatIdent {
                attrs,
                by_ref,
                mutability,
                ident: db_ident,
                subpat,
            } = db_pat_ident;

            if !attrs.is_empty() {
                return Err(syn::Error::new_spanned(
                    db_pat_ident,
                    "database parameter cannot have attributes",
                ));
            }

            if by_ref.is_some() {
                return Err(syn::Error::new_spanned(
                    by_ref,
                    "database parameter cannot be borrowed",
                ));
            }

            if mutability.is_some() {
                return Err(syn::Error::new_spanned(
                    mutability,
                    "database parameter cannot be mutable",
                ));
            }

            if let Some((at, _)) = subpat {
                return Err(syn::Error::new_spanned(
                    at,
                    "database parameter cannot have a subpattern",
                ));
            }

            let tykind_error_msg =
                "must have type `&dyn Db`, where `Db` is some Salsa Database trait";

            let syn::Type::Reference(ref_type) = &*typed.ty else {
                return Err(syn::Error::new(typed.ty.span(), tykind_error_msg));
            };

            if let Some(lt) = explicit_lt {
                if ref_type.lifetime.is_none() {
                    return Err(syn::Error::new_spanned(
                        ref_type.and_token,
                        format!("must have a `{}` lifetime", lt.lifetime.to_token_stream()),
                    ));
                }
            }

            let extract_db_path = || -> Result<&'arg syn::Path, Span> {
                if let Some(m) = &ref_type.mutability {
                    return Err(m.span());
                }

                let syn::Type::TraitObject(d) = &*ref_type.elem else {
                    return Err(ref_type.span());
                };

                if d.bounds.len() != 1 {
                    return Err(d.span());
                }

                let syn::TypeParamBound::Trait(syn::TraitBound {
                    paren_token,
                    modifier,
                    lifetimes,
                    path,
                }) = &d.bounds[0]
                else {
                    return Err(d.span());
                };

                if let Some(p) = paren_token {
                    return Err(p.span.open());
                }

                let syn::TraitBoundModifier::None = modifier else {
                    return Err(d.span());
                };

                if let Some(lt) = lifetimes {
                    return Err(lt.span());
                }

                Ok(path)
            };

            let db_path =
                extract_db_path().map_err(|span| syn::Error::new(span, tykind_error_msg))?;

            Ok((db_ident, db_path))
        }
    }
}
