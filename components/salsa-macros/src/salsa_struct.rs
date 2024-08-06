//! Common code for `#[salsa::interned]`, `#[salsa::input]`, and
//! `#[salsa::tracked]` decorators.
//!
//! Example of usage:
//!
//! ```rust,ignore
//! #[salsa::interned(jar = Jar0, data = TyData0)]
//! #[derive(Eq, PartialEq, Hash, Debug, Clone)]
//! struct Ty0 {
//!    field1: Type1,
//!    #[ref] field2: Type2,
//!    ...
//! }
//! ```
//! For an interned or entity struct `Foo`, we generate:
//!
//! * the actual struct: `struct Foo(Id);`
//! * constructor function: `impl Foo { fn new(db: &crate::Db, field1: Type1, ..., fieldN: TypeN) -> Self { ... } }
//! * field accessors: `impl Foo { fn field1(&self) -> Type1 { self.field1.clone() } }`
//!     * if the field is `ref`, we generate `fn field1(&self) -> &Type1`
//!
//! Only if there are no `ref` fields:
//!
//! * the data type: `struct FooData { field1: Type1, ... }` or `enum FooData { ... }`
//! * data method `impl Foo { fn data(&self, db: &dyn crate::Db) -> FooData { FooData { f: self.f(db), ... } } }`
//!     * this could be optimized, particularly for interned fields

use crate::{
    db_lifetime,
    options::{AllowedOptions, Options},
};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use syn::spanned::Spanned;

pub(crate) struct SalsaStruct<'s, A: SalsaStructAllowedOptions> {
    struct_item: &'s syn::ItemStruct,
    args: &'s Options<A>,
    fields: Vec<SalsaField<'s>>,
}

pub(crate) trait SalsaStructAllowedOptions: AllowedOptions {
    /// The kind of struct (e.g., interned, input, tracked).
    const KIND: &'static str;

    /// Are `#[id]` fields allowed?
    const ALLOW_ID: bool;

    /// Does this kind of struct have a `'db` lifetime?
    const HAS_LIFETIME: bool;

    /// Are `#[default]` fields allowed?
    const ALLOW_DEFAULT: bool;
}

pub(crate) struct SalsaField<'s> {
    field: &'s syn::Field,

    pub(crate) has_id_attr: bool,
    pub(crate) has_default_attr: bool,
    pub(crate) has_ref_attr: bool,
    pub(crate) has_no_eq_attr: bool,
    get_name: syn::Ident,
    set_name: syn::Ident,
}

const BANNED_FIELD_NAMES: &[&str] = &["from", "new"];

#[allow(clippy::type_complexity)]
pub(crate) const FIELD_OPTION_ATTRIBUTES: &[(&str, fn(&syn::Attribute, &mut SalsaField))] = &[
    ("id", |_, ef| ef.has_id_attr = true),
    ("default", |_, ef| ef.has_default_attr = true),
    ("return_ref", |_, ef| ef.has_ref_attr = true),
    ("no_eq", |_, ef| ef.has_no_eq_attr = true),
    ("get", |attr, ef| {
        ef.get_name = attr.parse_args().unwrap();
    }),
    ("set", |attr, ef| {
        ef.set_name = attr.parse_args().unwrap();
    }),
];

impl<'s, A> SalsaStruct<'s, A>
where
    A: SalsaStructAllowedOptions,
{
    pub fn new(struct_item: &'s syn::ItemStruct, args: &'s Options<A>) -> syn::Result<Self> {
        let syn::Fields::Named(n) = &struct_item.fields else {
            return Err(syn::Error::new_spanned(
                &struct_item.ident,
                "must have named fields for a struct",
            ));
        };

        let fields = n
            .named
            .iter()
            .map(SalsaField::new)
            .collect::<syn::Result<_>>()?;

        let this = Self {
            struct_item,
            args,
            fields,
        };

        this.maybe_disallow_id_fields()?;
        this.maybe_disallow_default_fields()?;

        this.check_generics()?;

        Ok(this)
    }

    /// Returns the `constructor_name` in `Options` if it is `Some`, else `new`
    pub(crate) fn constructor_name(&self) -> syn::Ident {
        match self.args.constructor_name.clone() {
            Some(name) => name,
            None => Ident::new("new", self.struct_item.span()),
        }
    }

    /// Disallow `#[id]` attributes on the fields of this struct.
    ///
    /// If an `#[id]` field is found, return an error.
    ///
    /// # Parameters
    ///
    /// * `kind`, the attribute name (e.g., `input` or `interned`)
    fn maybe_disallow_id_fields(&self) -> syn::Result<()> {
        if A::ALLOW_ID {
            return Ok(());
        }

        // Check if any field has the `#[id]` attribute.
        for ef in &self.fields {
            if ef.has_id_attr {
                return Err(syn::Error::new_spanned(
                    ef.field,
                    format!("`#[id]` cannot be used with `#[salsa::{}]`", A::KIND),
                ));
            }
        }

        Ok(())
    }

    /// Disallow `#[default]` attributes on the fields of this struct.
    ///
    /// If an `#[default]` field is found, return an error.
    ///
    /// # Parameters
    ///
    /// * `kind`, the attribute name (e.g., `input` or `interned`)
    fn maybe_disallow_default_fields(&self) -> syn::Result<()> {
        if A::ALLOW_DEFAULT {
            return Ok(());
        }

        // Check if any field has the `#[id]` attribute.
        for ef in &self.fields {
            if ef.has_default_attr {
                return Err(syn::Error::new_spanned(
                    ef.field,
                    format!("`#[id]` cannot be used with `#[salsa::{}]`", A::KIND),
                ));
            }
        }

        Ok(())
    }

    /// Check that the generic parameters look as expected for this kind of struct.
    fn check_generics(&self) -> syn::Result<()> {
        if A::HAS_LIFETIME {
            db_lifetime::require_db_lifetime(&self.struct_item.generics)
        } else {
            db_lifetime::require_no_generics(&self.struct_item.generics)
        }
    }

    pub(crate) fn field_ids(&self) -> Vec<&syn::Ident> {
        self.fields
            .iter()
            .map(|f| f.field.ident.as_ref().unwrap())
            .collect()
    }

    pub(crate) fn field_indices(&self) -> Vec<Literal> {
        (0..self.fields.len())
            .map(Literal::usize_unsuffixed)
            .collect()
    }

    pub(crate) fn num_fields(&self) -> Literal {
        Literal::usize_unsuffixed(self.fields.len())
    }

    pub(crate) fn id_field_indices(&self) -> Vec<Literal> {
        self.fields
            .iter()
            .zip(0..)
            .filter_map(|(f, index)| if f.has_id_attr { Some(index) } else { None })
            .map(Literal::usize_unsuffixed)
            .collect()
    }

    pub(crate) fn required_fields(&self) -> Vec<TokenStream> {
        self.fields
            .iter()
            .filter_map(|f| {
                if f.has_default_attr {
                    None
                } else {
                    let ident = f.field.ident.as_ref().unwrap();
                    let ty = &f.field.ty;
                    Some(quote!(#ident #ty))
                }
            })
            .collect()
    }

    pub(crate) fn field_vis(&self) -> Vec<&syn::Visibility> {
        self.fields.iter().map(|f| &f.field.vis).collect()
    }

    pub(crate) fn field_getter_ids(&self) -> Vec<&syn::Ident> {
        self.fields.iter().map(|f| &f.get_name).collect()
    }

    pub(crate) fn field_setter_ids(&self) -> Vec<&syn::Ident> {
        self.fields.iter().map(|f| &f.set_name).collect()
    }

    pub(crate) fn field_durability_ids(&self) -> Vec<syn::Ident> {
        self.fields
            .iter()
            .map(|f| quote::format_ident!("{}_durability", f.field.ident.as_ref().unwrap()))
            .collect()
    }

    pub(crate) fn field_tys(&self) -> Vec<&syn::Type> {
        self.fields.iter().map(|f| &f.field.ty).collect()
    }

    pub(crate) fn field_options(&self) -> Vec<TokenStream> {
        self.fields
            .iter()
            .map(|f| {
                let clone_ident = if f.has_ref_attr {
                    syn::Ident::new("no_clone", Span::call_site())
                } else {
                    syn::Ident::new("clone", Span::call_site())
                };

                let backdate_ident = if f.has_no_eq_attr {
                    syn::Ident::new("no_backdate", Span::call_site())
                } else {
                    syn::Ident::new("backdate", Span::call_site())
                };

                let default_ident = if f.has_default_attr {
                    syn::Ident::new("default", Span::call_site())
                } else {
                    syn::Ident::new("required", Span::call_site())
                };

                quote!((#clone_ident, #backdate_ident, #default_ident))
            })
            .collect()
    }

    pub fn generate_debug_impl(&self) -> bool {
        self.args.no_debug.is_none()
    }
}

impl<'s> SalsaField<'s> {
    fn new(field: &'s syn::Field) -> syn::Result<Self> {
        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        if BANNED_FIELD_NAMES.iter().any(|n| *n == field_name_str) {
            return Err(syn::Error::new(
                field_name.span(),
                format!(
                    "the field name `{}` is disallowed in salsa structs",
                    field_name_str
                ),
            ));
        }

        let get_name = Ident::new(&field_name_str, field_name.span());
        let set_name = Ident::new(&format!("set_{}", field_name_str), field_name.span());
        let mut result = SalsaField {
            field,
            has_id_attr: false,
            has_ref_attr: false,
            has_default_attr: false,
            has_no_eq_attr: false,
            get_name,
            set_name,
        };

        // Scan the attributes and look for the salsa attributes:
        for attr in &field.attrs {
            for (fa, func) in FIELD_OPTION_ATTRIBUTES {
                if attr.path().is_ident(fa) {
                    func(attr, &mut result);
                }
            }
        }

        Ok(result)
    }
}
