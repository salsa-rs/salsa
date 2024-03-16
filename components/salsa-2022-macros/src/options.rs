use std::marker::PhantomData;

use syn::{ext::IdentExt, spanned::Spanned};

/// "Options" are flags that can be supplied to the various salsa related
/// macros. They are listed like `(ref, no_eq, foo=bar)` etc. The commas
/// are required and trailing commas are permitted. The options accepted
/// for any particular location are configured via the `AllowedOptions`
/// trait.
pub(crate) struct Options<A: AllowedOptions> {
    /// The `return_ref` option is used to signal that field/return type is "by ref"
    ///
    /// If this is `Some`, the value is the `ref` identifier.
    pub return_ref: Option<syn::Ident>,

    ///  The `no_eq` option is used to signal that a given field does not implement
    /// the `Eq` trait and cannot be compared for equality.
    ///
    /// If this is `Some`, the value is the `no_eq` identifier.
    pub no_eq: Option<syn::Ident>,

    /// The `singleton` option is used on input with only one field
    /// It allows the creation of convenient methods
    pub singleton: Option<syn::Ident>,

    /// The `specify` option is used to signal that a tracked function can
    /// have its value externally specified (at least some of the time).
    ///
    /// If this is `Some`, the value is the `specify` identifier.
    pub specify: Option<syn::Ident>,

    /// The `jar = <type>` option is used to indicate the jar; it defaults to `crate::jar`.
    ///
    /// If this is `Some`, the value is the `<type>`.
    pub jar_ty: Option<syn::Type>,

    /// The `db = <path>` option is used to indicate the db.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub db_path: Option<syn::Path>,

    /// The `recovery_fn = <path>` option is used to indicate the recovery function.
    ///
    /// If this is `Some`, the value is the `<path>`.
    pub recovery_fn: Option<syn::Path>,

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

    /// The `destructor = <ident>` option lets the user specify the name of
    /// the destructor of a salsa struct. The destructor is only created for
    /// non-singleton inputs.
    ///
    /// If this is `Some`, the value is the `<ident>`.
    pub destructor_name: Option<syn::Ident>,

    /// Remember the `A` parameter, which plays no role after parsing.
    phantom: PhantomData<A>,
}

impl<A: AllowedOptions> Default for Options<A> {
    fn default() -> Self {
        Self {
            return_ref: Default::default(),
            specify: Default::default(),
            no_eq: Default::default(),
            jar_ty: Default::default(),
            db_path: Default::default(),
            recovery_fn: Default::default(),
            data: Default::default(),
            constructor_name: Default::default(),
            destructor_name: Default::default(),
            phantom: Default::default(),
            lru: Default::default(),
            singleton: Default::default(),
        }
    }
}

/// These flags determine which options are allowed in a given context
pub(crate) trait AllowedOptions {
    const RETURN_REF: bool;
    const SPECIFY: bool;
    const NO_EQ: bool;
    const SINGLETON: bool;
    const JAR: bool;
    const DATA: bool;
    const DB: bool;
    const RECOVERY_FN: bool;
    const LRU: bool;
    const CONSTRUCTOR_NAME: bool;
    const DESTRUCTOR_NAME: bool;
}

type Equals = syn::Token![=];
type Comma = syn::Token![,];

impl<A: AllowedOptions> Options<A> {
    /// Returns the `jar type` given by the user; if none is given,
    /// returns the default `crate::Jar`.
    pub(crate) fn jar_ty(&self) -> syn::Type {
        if let Some(jar_ty) = &self.jar_ty {
            return jar_ty.clone();
        }

        parse_quote! {crate::Jar}
    }

    pub(crate) fn should_backdate(&self) -> bool {
        self.no_eq.is_none()
    }
}

impl<A: AllowedOptions> syn::parse::Parse for Options<A> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut options = Options::default();

        while !input.is_empty() {
            let ident: syn::Ident = syn::Ident::parse_any(input)?;
            if ident == "return_ref" {
                if A::RETURN_REF {
                    if let Some(old) = std::mem::replace(&mut options.return_ref, Some(ident)) {
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
                    if let Some(old) = std::mem::replace(&mut options.no_eq, Some(ident)) {
                        return Err(syn::Error::new(old.span(), "option `no_eq` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`no_eq` option not allowed here",
                    ));
                }
            } else if ident == "singleton" {
                if A::SINGLETON {
                    if let Some(old) = std::mem::replace(&mut options.singleton, Some(ident)) {
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
                    if let Some(old) = std::mem::replace(&mut options.specify, Some(ident)) {
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
            } else if ident == "jar" {
                if A::JAR {
                    let _eq = Equals::parse(input)?;
                    let ty = syn::Type::parse(input)?;
                    if let Some(old) = std::mem::replace(&mut options.jar_ty, Some(ty)) {
                        return Err(syn::Error::new(old.span(), "option `jar` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`jar` option not allowed here",
                    ));
                }
            } else if ident == "db" {
                if A::DB {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = std::mem::replace(&mut options.db_path, Some(path)) {
                        return Err(syn::Error::new(old.span(), "option `db` provided twice"));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`db` option not allowed here",
                    ));
                }
            } else if ident == "recovery_fn" {
                if A::RECOVERY_FN {
                    let _eq = Equals::parse(input)?;
                    let path = syn::Path::parse(input)?;
                    if let Some(old) = std::mem::replace(&mut options.recovery_fn, Some(path)) {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `recovery_fn` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`recovery_fn` option not allowed here",
                    ));
                }
            } else if ident == "data" {
                if A::DATA {
                    let _eq = Equals::parse(input)?;
                    let ident = syn::Ident::parse(input)?;
                    if let Some(old) = std::mem::replace(&mut options.data, Some(ident)) {
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
                    if let Some(old) = std::mem::replace(&mut options.lru, Some(value)) {
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
                    if let Some(old) = std::mem::replace(&mut options.constructor_name, Some(ident))
                    {
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
            } else if ident == "destructor" {
                if A::DESTRUCTOR_NAME {
                    let _eq = Equals::parse(input)?;
                    let ident = syn::Ident::parse(input)?;
                    if let Some(old) = std::mem::replace(&mut options.destructor_name, Some(ident))
                    {
                        return Err(syn::Error::new(
                            old.span(),
                            "option `destructor` provided twice",
                        ));
                    }
                } else {
                    return Err(syn::Error::new(
                        ident.span(),
                        "`destructor` option not allowed here",
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
