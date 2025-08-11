use proc_macro2::{Literal, Span, TokenStream};
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{Ident, ItemFn};

use crate::hygiene::Hygiene;
use crate::options::{AllowedOptions, AllowedPersistOptions, Options};
use crate::{db_lifetime, fn_util};

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

impl AllowedOptions for TrackedFn {
    const RETURNS: bool = true;

    const SPECIFY: bool = true;

    const NO_EQ: bool = true;

    const DEBUG: bool = false;

    const NO_LIFETIME: bool = false;

    const NON_UPDATE_RETURN_TYPE: bool = true;

    const SINGLETON: bool = false;

    const DATA: bool = false;

    const DB: bool = false;

    const CYCLE_FN: bool = true;

    const CYCLE_INITIAL: bool = true;

    const CYCLE_RESULT: bool = true;

    const LRU: bool = true;

    const CONSTRUCTOR_NAME: bool = false;

    const ID: bool = false;

    const REVISIONS: bool = false;

    const HEAP_SIZE: bool = true;

    const SELF_TY: bool = true;

    const PERSIST: AllowedPersistOptions = AllowedPersistOptions::AllowedIdent;
}

struct Macro {
    hygiene: Hygiene,
    args: FnArgs,
}

struct ValidFn<'item> {
    db_ident: &'item syn::Ident,
    db_path: &'item syn::Path,
}

const ALLOWED_RETURN_MODES: &[&str] = &["copy", "clone", "ref", "deref", "as_ref", "as_deref"];

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
        let interned_input_tys = input_tys.iter().map(|&ty| {
            let mut ty = ty.clone();
            syn::visit_mut::visit_type_mut(
                &mut ToDbLifetimeVisitor {
                    db_lifetime: db_lt.clone(),
                },
                &mut ty,
            );
            ty
        });
        let output_ty = self.output_ty(&db_lt, &item)?;
        let (cycle_recovery_fn, cycle_recovery_initial, cycle_recovery_strategy) =
            self.cycle_recovery()?;
        let is_specifiable = self.args.specify.is_some();
        let requires_update = self.args.non_update_return_type.is_none();
        let heap_size_fn = self.args.heap_size_fn.iter();
        let eq = if let Some(token) = &self.args.no_eq {
            if self.args.cycle_fn.is_some() {
                return Err(syn::Error::new_spanned(
                    token,
                    "the `no_eq` option cannot be used with `cycle_fn`",
                ));
            }
            quote!(false)
        } else {
            quote_spanned!(output_ty.span() =>
                old_value == new_value
            )
        };
        // we need to generate the entire function here
        // as the locals (parameters) will have def site hygiene otherwise
        // if emitted in the decl macro
        let eq = quote! {
            fn values_equal<#db_lt>(
                old_value: &Self::Output<#db_lt>,
                new_value: &Self::Output<#db_lt>,
            ) -> bool {
                #eq
            }
        };

        let mut inner_fn = item.clone();
        inner_fn.vis = syn::Visibility::Inherited;
        inner_fn.sig.ident = self.hygiene.ident("inner");

        let zalsa = self.hygiene.ident("zalsa");
        let Configuration = self.hygiene.scoped_ident(fn_name, "Configuration");
        let InternedData = self.hygiene.scoped_ident(fn_name, "InternedData");
        let FN_CACHE = self.hygiene.scoped_ident(fn_name, "FN_CACHE");
        let INTERN_CACHE = self.hygiene.scoped_ident(fn_name, "INTERN_CACHE");
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

        let return_mode = self
            .args
            .returns
            .clone()
            .unwrap_or(Ident::new("clone", Span::call_site()));

        // Validate return mode
        if !ALLOWED_RETURN_MODES
            .iter()
            .any(|mode| mode == &return_mode.to_string())
        {
            return Err(syn::Error::new(
                return_mode.span(),
                format!("Invalid return mode. Allowed modes are: {ALLOWED_RETURN_MODES:?}"),
            ));
        }

        let persist = self.args.persist();

        // The path expression is responsible for emitting the primary span in the diagnostic we
        // want, so by uniformly using `output_ty.span()` we ensure that the diagnostic is emitted
        // at the return type in the original input.
        // See the tests/compile-fail/tracked_fn_return_ref.rs test
        let maybe_update_path = quote_spanned! {output_ty.span() =>
            UpdateDispatch::<#output_ty>::maybe_update
        };
        let assert_return_type_is_update = if requires_update {
            quote! {
                #[allow(clippy::all, warnings)]
                fn _assert_return_type_is_update<#db_lt>()  {
                    use #zalsa::{UpdateFallback, UpdateDispatch};
                    #maybe_update_path;
                }
            }
        } else {
            quote! {}
        };
        let self_ty = match &self.args.self_ty {
            Some(ty) => quote! { self_ty: #ty, },
            None => quote! {},
        };

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
                interned_input_tys: [#(#interned_input_tys),*],
                output_ty: #output_ty,
                inner_fn: { #inner_fn },
                cycle_recovery_fn: #cycle_recovery_fn,
                cycle_recovery_initial: #cycle_recovery_initial,
                cycle_recovery_strategy: #cycle_recovery_strategy,
                is_specifiable: #is_specifiable,
                values_equal: {#eq},
                needs_interner: #needs_interner,
                heap_size_fn: #(#heap_size_fn)*,
                lru: #lru,
                return_mode: #return_mode,
                persist: #persist,
                assert_return_type_is_update: { #assert_return_type_is_update },
                #self_ty
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
        // TODO should we ask the user to specify a struct that impls a trait with two methods,
        // rather than asking for two methods separately?
        match (
            &self.args.cycle_fn,
            &self.args.cycle_initial,
            &self.args.cycle_result,
        ) {
            (Some(cycle_fn), Some(cycle_initial), None) => Ok((
                quote!((#cycle_fn)),
                quote!((#cycle_initial)),
                quote!(Fixpoint),
            )),
            (None, None, None) => Ok((
                quote!((salsa::plumbing::unexpected_cycle_recovery!)),
                quote!((salsa::plumbing::unexpected_cycle_initial!)),
                quote!(Panic),
            )),
            (Some(_), None, None) => Err(syn::Error::new_spanned(
                self.args.cycle_fn.as_ref().unwrap(),
                "must provide `cycle_initial` along with `cycle_fn`",
            )),
            (None, Some(_), None) => Err(syn::Error::new_spanned(
                self.args.cycle_initial.as_ref().unwrap(),
                "must provide `cycle_fn` along with `cycle_initial`",
            )),
            (None, None, Some(cycle_result)) => Ok((
                quote!((salsa::plumbing::unexpected_cycle_recovery!)),
                quote!((#cycle_result)),
                quote!(FallbackImmediate),
            )),
            (_, _, Some(_)) => Err(syn::Error::new_spanned(
                self.args.cycle_initial.as_ref().unwrap(),
                "must provide either `cycle_result` or `cycle_fn` & `cycle_initial`, not both",
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

struct ToDbLifetimeVisitor {
    db_lifetime: syn::Lifetime,
}

impl syn::visit_mut::VisitMut for ToDbLifetimeVisitor {
    fn visit_lifetime_mut(&mut self, i: &mut syn::Lifetime) {
        i.clone_from(&self.db_lifetime);
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
