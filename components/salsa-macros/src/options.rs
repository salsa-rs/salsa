use std::marker::PhantomData;

use syn::ext::IdentExt;
use syn::parenthesized;
use syn::spanned::Spanned;

/// "Options" are flags that can be supplied to the various salsa related
/// macros. They are listed like `(ref, no_eq, foo=bar)` etc. The commas
/// are required and trailing commas are permitted. The options accepted
/// for any particular location are configured via the `AllowedOptions`
/// trait.
#[derive(Debug)]
pub(crate) struct Options<A: AllowedOptions> {
    /// The `returns` option is used to configure the "return mode" for the field/function.
    /// This may be one of `copy`, `clone`, `ref`, `as_ref`, `as_deref`.
    ///
    /// If this is `Some`, the value is the ident representing the selected mode.
    pub returns: Option<syn::Ident>,

    /// The `no_eq` option is used to signal that a given field does not implement
    /// the `Eq` trait and cannot be compared for equality.
    ///
    /// If this is `Some`, the value is the `no_eq` identifier.
    pub no_eq: Option<syn::Ident>,

    /// Signal we should generate a `Debug` impl.
    ///
    /// If this is `Some`, the value is the `debug` identifier.
    pub debug: Option<syn::Ident>,

    /// Signal we should not include the `'db` lifetime.
    ///
    /// If this is `Some`, the value is the `no_lifetime` identifier.
    pub no_lifetime: Option<syn::Ident>,

    /// The `singleton` option is used on input with only one field
    /// It allows the creation of convenient methods
    pub singleton: Option<syn::Ident>,

    /// The `specify` option is used to signal that a tracked function can
    /// have its value externally specified (at least some of the time).
    ///
    /// If this is `Some`, the value is the `specify` identifier.
    pub specify: Option<syn::Ident>,

    /// The `non_update_return_type` option is used to signal that a tracked function's
    /// return type does not require `Update` to be implemented. This is unsafe and
    /// generally discouraged as it allows for dangling references.
    ///
    /// If this is `Some`, the value is the `non_update_return_type` identifier.
    pub non_update_return_type: Option<syn::Ident>,

    /// The `db = <path>` option is used to indicate the db.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub db_path: Option<syn::Path>,

    /// The `cycle_fn = <path>` option is used to indicate the cycle recovery function.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub cycle_fn: Option<syn::Path>,

    /// The `cycle_initial = <path>` option is the initial value for cycle iteration.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub cycle_initial: Option<syn::Path>,

    /// The `cycle_result = <path>` option is the result for non-fixpoint cycle.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub cycle_result: Option<syn::Expr>,

    /// The `data = <ident>` option is used to define the name of the data type for an interned
    /// struct.
    ///
    /// If this is `Some`, the value is the `<ident>`.
    pub data: Option<syn::Ident>,

    /// The `lru = <usize>` option is used to set the lru capacity for a tracked function.
    ///
    /// If this is `Some`, the value is the `<usize>`.
    pub lru: Option<usize>,

    /// The `constructor = <ident>` option lets the user specify the name of
    /// the constructor of a salsa struct.
    ///
    /// If this is `Some`, the value is the `<ident>`.
    pub constructor_name: Option<syn::Ident>,

    /// The `id = <path>` option is used to set a custom ID for interrned structs.
    ///
    /// The ID must implement `salsa::plumbing::AsId` and `salsa::plumbing::FromId`.
    /// If this is `Some`, the value is the `<ident>`.
    pub id: Option<syn::Path>,

    /// The `revisions = <usize>` option is used to set the minimum number of revisions
    /// to keep a value interned.
    ///
    /// This is stored as a `syn::Expr` to support `usize::MAX`.
    pub revisions: Option<syn::Expr>,

    /// The `heap_size = <path>` option can be used to track heap memory usage of memoized
    /// values.
    ///
    /// If this is `Some`, the value is the provided `heap_size` function.
    pub heap_size_fn: Option<syn::Path>,

    /// The `self_ty = <Ty>` option is used to set the the self type of the tracked impl for tracked
    /// functions. This is merely used to refine the query name.
    pub self_ty: Option<syn::Type>,

    /// Remember the `A` parameter, which plays no role after parsing.
    phantom: PhantomData<A>,
}

impl<A: AllowedOptions> Default for Options<A> {
    fn default() -> Self {
        Self {
            returns: Default::default(),
            specify: Default::default(),
            non_update_return_type: Default::default(),
            no_eq: Default::default(),
            debug: Default::default(),
            no_lifetime: Default::default(),
            db_path: Default::default(),
            cycle_fn: Default::default(),
            cycle_initial: Default::default(),
            cycle_result: Default::default(),
            data: Default::default(),
            constructor_name: Default::default(),
            phantom: Default::default(),
            lru: Default::default(),
            singleton: Default::default(),
            id: Default::default(),
            revisions: Default::default(),
            heap_size_fn: Default::default(),
            self_ty: Default::default(),
        }
    }
}

/// These flags determine which options are allowed in a given context
pub(crate) trait AllowedOptions {
    const RETURNS: bool;
    const SPECIFY: bool;
    const NO_EQ: bool;
    const DEBUG: bool;
    const NO_LIFETIME: bool;
    const NON_UPDATE_RETURN_TYPE: bool;
    const SINGLETON: bool;
    const DATA: bool;
    const DB: bool;
    const CYCLE_FN: bool;
    const CYCLE_INITIAL: bool;
    const CYCLE_RESULT: bool;
    const LRU: bool;
    const CONSTRUCTOR_NAME: bool;
    const ID: bool;
    const REVISIONS: bool;
    const HEAP_SIZE: bool;
    const SELF_TY: bool;
}

type Equals = syn::Token![=];
type Comma = syn::Token![,];

impl<A: AllowedOptions> syn::parse::Parse for Options<A> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut options = Options::default();

        while !input.is_empty() {
            let ident: syn::Ident = syn::Ident::parse_any(input)?;
            if ident == "returns" {
                let content;
                parenthesized!(content in input);
                let mode = syn::Ident::parse_any(&content)?;
                if A::RETURNS {
                    if let Some(old) = options.returns.replace(mode) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `returns` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`returns` option not allowed here",
                    ));
                }
            } else if ident == "no_eq" {
                if A::NO_EQ {
                    if let Some(old) = options.no_eq.replace(ident) {
                        return Err(syn::Error::new(old.span(), "option `no_eq` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`no_eq` option not allowed here",
                    ));
                }
            } else if ident == "debug" {
                if A::DEBUG {
                    if let Some(old) = options.debug.replace(ident) {
                        return Err(syn::Error::new(old.span(), "option `debug` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`debug` option not allowed here",
                    ));
                }
            } else if ident == "no_lifetime" {
                if A::NO_LIFETIME {
                    if let Some(old) = options.no_lifetime.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `no_lifetime` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`no_lifetime` option not allowed here",
                    ));
                }
            } else if ident == "unsafe" {
                if A::NON_UPDATE_RETURN_TYPE {
                    let content;
                    parenthesized!(content in input);
                    let ident = syn::Ident::parse_any(&content)?;
                    if ident == "non_update_return_type" {
                        if let Some(old) = options.non_update_return_type.replace(ident) {
                            return Err(syn::Error::new(
                                old.span(),
                                "option `non_update_return_type` provided twice",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new(
                            ident.span(),
                            "expected `non_update_return_type`",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`unsafe` options not allowed here",
                    ));
                }
            } else if ident == "singleton" {
                if A::SINGLETON {
                    if let Some(old) = options.singleton.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `singleton` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`singleton` option not allowed here",
                    ));
                }
            } else if ident == "specify" {
                if A::SPECIFY {
                    if let Some(old) = options.specify.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `specify` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`specify` option not allowed here",
                    ));
                }
            } else if ident == "db" {
                if A::DB {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = options.db_path.replace(path) {
                        return Err(syn::Error::new(old.span(), "option `db` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`db` option not allowed here",
                    ));
                }
            } else if ident == "cycle_fn" {
                if A::CYCLE_FN {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = options.cycle_fn.replace(path) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `cycle_fn` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`cycle_fn` option not allowed here",
                    ));
                }
            } else if ident == "cycle_initial" {
                if A::CYCLE_INITIAL {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = options.cycle_initial.replace(path) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `cycle_initial` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`cycle_initial` option not allowed here",
                    ));
                }
            } else if ident == "cycle_result" {
                if A::CYCLE_RESULT {
                    let _eq = Equals::parse(input)?;
                    let expr = syn::Expr::parse(input)?;
                    if let Some(old) = options.cycle_result.replace(expr) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `cycle_result` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`cycle_result` option not allowed here",
                    ));
                }
            } else if ident == "data" {
                if A::DATA {
                    let _eq = Equals::parse(input)?;
                    let ident = syn::Ident::parse(input)?;
                    if let Some(old) = options.data.replace(ident) {
                        return Err(syn::Error::new(old.span(), "option `data` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`data` option not allowed here",
                    ));
                }
            } else if ident == "lru" {
                if A::LRU {
                    let _eq = Equals::parse(input)?;
                    let lit = syn::LitInt::parse(input)?;
                    let value = lit.base10_parse::<usize>()?;
                    if let Some(old) = options.lru.replace(value) {
                        return Err(syn::Error::new(old.span(), "option `lru` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`lru` option not allowed here",
                    ));
                }
            } else if ident == "constructor" {
                if A::CONSTRUCTOR_NAME {
                    let _eq = Equals::parse(input)?;
                    let ident = syn::Ident::parse(input)?;
                    if let Some(old) = options.constructor_name.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `constructor` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`constructor` option not allowed here",
                    ));
                }
            } else if ident == "id" {
                if A::ID {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    options.id = Some(path);
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`id` option not allowed here",
                    ));
                }
            } else if ident == "revisions" {
                if A::REVISIONS {
                    let _eq = Equals::parse(input)?;
                    let expr = syn::Expr::parse(input)?;
                    if let Some(old) = options.revisions.replace(expr) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `revisions` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`revisions` option not allowed here",
                    ));
                }
            } else if ident == "heap_size" {
                if A::HEAP_SIZE {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = options.heap_size_fn.replace(path) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `heap_size` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`heap_size` option not allowed here",
                    ));
                }
            } else if ident == "self_ty" {
                if A::SELF_TY {
                    let _eq = Equals::parse(input)?;
                    let ty = syn::Type::parse(input)?;
                    if let Some(old) = options.self_ty.replace(ty) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `self_ty` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`self_ty` option not allowed here",
                    ));
                }
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unrecognized option `{ident}`"),
                ));
            }

            if input.is_empty() {
                break;
            }

            let _comma = Comma::parse(input)?;
        }

        Ok(options)
    }
}
impl<A: AllowedOptions> quote::ToTokens for Options<A> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let Self {
            returns,
            no_eq,
            debug,
            no_lifetime,
            singleton,
            specify,
            non_update_return_type,
            db_path,
            cycle_fn,
            cycle_initial,
            cycle_result,
            data,
            lru,
            constructor_name,
            id,
            revisions,
            heap_size_fn,
            self_ty,
            phantom: _,
        } = self;
        if let Some(returns) = returns {
            tokens.extend(quote::quote! { returns(#returns), });
        };
        if no_eq.is_some() {
            tokens.extend(quote::quote! { no_eq, });
        }
        if debug.is_some() {
            tokens.extend(quote::quote! { debug, });
        }
        if no_lifetime.is_some() {
            tokens.extend(quote::quote! { no_lifetime, });
        }
        if singleton.is_some() {
            tokens.extend(quote::quote! { singleton, });
        }
        if specify.is_some() {
            tokens.extend(quote::quote! { specify, });
        }
        if non_update_return_type.is_some() {
            tokens.extend(quote::quote! { unsafe(non_update_return_type), });
        }
        if let Some(db_path) = db_path {
            tokens.extend(quote::quote! { db = #db_path, });
        }
        if let Some(cycle_fn) = cycle_fn {
            tokens.extend(quote::quote! { cycle_fn = #cycle_fn, });
        }
        if let Some(cycle_initial) = cycle_initial {
            tokens.extend(quote::quote! { cycle_initial = #cycle_initial, });
        }
        if let Some(cycle_result) = cycle_result {
            tokens.extend(quote::quote! { cycle_result = #cycle_result, });
        }
        if let Some(data) = data {
            tokens.extend(quote::quote! { data = #data, });
        }
        if let Some(lru) = lru {
            tokens.extend(quote::quote! { lru = #lru, });
        }
        if let Some(constructor_name) = constructor_name {
            tokens.extend(quote::quote! { constructor = #constructor_name, });
        }
        if let Some(id) = id {
            tokens.extend(quote::quote! { id = #id, });
        }
        if let Some(revisions) = revisions {
            tokens.extend(quote::quote! { revisions = #revisions, });
        }
        if let Some(heap_size_fn) = heap_size_fn {
            tokens.extend(quote::quote! { heap_size = #heap_size_fn, });
        }
        if let Some(self_ty) = self_ty {
            tokens.extend(quote::quote! { self_ty = #self_ty, });
        }
    }
}
