use std::marker::PhantomData;

use syn::ext::IdentExt;
use syn::spanned::Spanned;

/// "Options" are flags that can be supplied to the various salsa related
/// macros. They are listed like `(ref, no_eq, foo=bar)` etc. The commas
/// are required and trailing commas are permitted. The options accepted
/// for any particular location are configured via the `AllowedOptions`
/// trait.
#[derive(Debug)]
pub(crate) struct Options<A: AllowedOptions> {
    /// The `return_ref` option is used to signal that field/return type is "by ref"
    ///
    /// If this is `Some`, the value is the `ref` identifier.
    pub return_ref: Option<syn::Ident>,

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

    /// Signal we should not generate a `Clone` impl.
    ///
    /// If this is `Some`, the value is the `no_clone` identifier.
    pub no_clone: Option<syn::Ident>,

    /// The `singleton` option is used on input with only one field
    /// It allows the creation of convenient methods
    pub singleton: Option<syn::Ident>,

    /// The `specify` option is used to signal that a tracked function can
    /// have its value externally specified (at least some of the time).
    ///
    /// If this is `Some`, the value is the `specify` identifier.
    pub specify: Option<syn::Ident>,

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

    /// Remember the `A` parameter, which plays no role after parsing.
    phantom: PhantomData<A>,
}

impl<A: AllowedOptions> Default for Options<A> {
    fn default() -> Self {
        Self {
            return_ref: Default::default(),
            specify: Default::default(),
            no_eq: Default::default(),
            debug: Default::default(),
            no_lifetime: Default::default(),
            no_clone: Default::default(),
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
        }
    }
}

/// These flags determine which options are allowed in a given context
pub(crate) trait AllowedOptions {
    const RETURN_REF: bool;
    const SPECIFY: bool;
    const NO_EQ: bool;
    const DEBUG: bool;
    const NO_LIFETIME: bool;
    const NO_CLONE: bool;
    const SINGLETON: bool;
    const DATA: bool;
    const DB: bool;
    const CYCLE_FN: bool;
    const CYCLE_INITIAL: bool;
    const CYCLE_RESULT: bool;
    const LRU: bool;
    const CONSTRUCTOR_NAME: bool;
    const ID: bool;
}

type Equals = syn::Token![=];
type Comma = syn::Token![,];

impl<A: AllowedOptions> syn::parse::Parse for Options<A> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut options = Options::default();

        while !input.is_empty() {
            let ident: syn::Ident = syn::Ident::parse_any(input)?;
            if ident == "return_ref" {
                if A::RETURN_REF {
                    if let Some(old) = options.return_ref.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `return_ref` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`return_ref` option not allowed here",
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
            } else if ident == "no_clone" {
                if A::NO_CLONE {
                    if let Some(old) = options.no_clone.replace(ident) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `no_clone` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`no_clone` option not allowed here",
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
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unrecognized option `{}`", ident),
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
