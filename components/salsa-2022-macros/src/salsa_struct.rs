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
    options::{AllowedOptions, Options},
    xform::ChangeLt,
};
use proc_macro2::{Ident, Span, TokenStream};
use syn::{
    punctuated::Punctuated, spanned::Spanned, token::Comma, GenericParam, ImplGenerics,
    TypeGenerics, WhereClause,
};

pub(crate) struct SalsaStruct<A: AllowedOptions> {
    args: Options<A>,
    struct_item: syn::ItemStruct,
    customizations: Vec<Customization>,
    fields: Vec<SalsaField>,
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum Customization {
    DebugWithDb,
}

const BANNED_FIELD_NAMES: &[&str] = &["from", "new"];

/// Classifies the kind of field stored in this salsa
/// struct.
#[derive(Debug, PartialEq, Eq)]
pub enum TheStructKind {
    /// Stores an "id"
    Id,

    /// Stores a "pointer" with the given lifetime
    Pointer(syn::Lifetime),
}

impl<A: AllowedOptions> SalsaStruct<A> {
    pub(crate) fn new(
        args: proc_macro::TokenStream,
        input: proc_macro::TokenStream,
    ) -> syn::Result<Self> {
        let struct_item = syn::parse(input)?;
        Self::with_struct(args, struct_item)
    }

    pub(crate) fn with_struct(
        args: proc_macro::TokenStream,
        struct_item: syn::ItemStruct,
    ) -> syn::Result<Self> {
        let args: Options<A> = syn::parse(args)?;
        let customizations = Self::extract_customizations(&struct_item)?;
        let fields = Self::extract_fields(&struct_item)?;
        Ok(Self {
            args,
            struct_item,
            customizations,
            fields,
        })
    }

    pub(crate) fn args(&self) -> &Options<A> {
        &self.args
    }

    pub(crate) fn require_no_generics(&self) -> syn::Result<()> {
        if let Some(param) = self.struct_item.generics.params.iter().next() {
            return Err(syn::Error::new_spanned(
                param,
                "generic parameters not allowed here",
            ));
        }

        Ok(())
    }

    pub(crate) fn require_db_lifetime(&self) -> syn::Result<()> {
        let generics = &self.struct_item.generics;

        if generics.params.len() == 0 {
            return Ok(());
        }

        for (param, index) in generics.params.iter().zip(0..) {
            let error = match param {
                syn::GenericParam::Lifetime(_) => index > 0,
                syn::GenericParam::Type(_) | syn::GenericParam::Const(_) => true,
            };

            if error {
                return Err(syn::Error::new_spanned(
                    param,
                    "only a single lifetime parameter is accepted",
                ));
            }
        }

        Ok(())
    }

    /// Some salsa structs require a "Configuration" struct
    /// because they make use of GATs. This function
    /// synthesizes a name and generates the struct declaration.
    pub(crate) fn config_struct(&self) -> syn::ItemStruct {
        let config_ident = syn::Ident::new(
            &format!("__{}Config", self.the_ident()),
            self.the_ident().span(),
        );
        let visibility = self.visibility();

        parse_quote! {
            #visibility struct #config_ident {
                _uninhabited: std::convert::Infallible,
            }
        }
    }

    pub(crate) fn the_struct_kind(&self) -> TheStructKind {
        if self.struct_item.generics.params.is_empty() {
            TheStructKind::Id
        } else {
            if let Some(lt) = self.struct_item.generics.lifetimes().next() {
                TheStructKind::Pointer(lt.lifetime.clone())
            } else {
                TheStructKind::Pointer(self.default_db_lifetime())
            }
        }
    }

    fn extract_customizations(struct_item: &syn::ItemStruct) -> syn::Result<Vec<Customization>> {
        Ok(struct_item
            .attrs
            .iter()
            .map(|attr| {
                if attr.path.is_ident("customize") {
                    // FIXME: this should be a comma separated list but I couldn't
                    // be bothered to remember how syn does this.
                    let args: syn::Ident = attr.parse_args()?;
                    if args.to_string() == "DebugWithDb" {
                        Ok(vec![Customization::DebugWithDb])
                    } else {
                        Err(syn::Error::new_spanned(args, "unrecognized customization"))
                    }
                } else {
                    Ok(vec![])
                }
            })
            .collect::<Result<Vec<Vec<_>>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    /// Extract out the fields and their options:
    /// If this is a struct, it must use named fields, so we can define field accessors.
    /// If it is an enum, then this is not necessary.
    fn extract_fields(struct_item: &syn::ItemStruct) -> syn::Result<Vec<SalsaField>> {
        match &struct_item.fields {
            syn::Fields::Named(n) => Ok(n
                .named
                .iter()
                .map(SalsaField::new)
                .collect::<syn::Result<Vec<_>>>()?),
            f => Err(syn::Error::new_spanned(
                f,
                "must have named fields for a struct",
            )),
        }
    }

    /// Iterator over all named fields.
    ///
    /// If this is an enum, empty iterator.
    pub(crate) fn all_fields(&self) -> impl Iterator<Item = &SalsaField> {
        self.fields.iter()
    }

    /// Names of all fields (id and value).
    ///
    /// If this is an enum, empty vec.
    pub(crate) fn all_field_names(&self) -> Vec<&syn::Ident> {
        self.all_fields().map(|ef| ef.name()).collect()
    }

    /// Visibilities of all fields
    pub(crate) fn all_field_vises(&self) -> Vec<&syn::Visibility> {
        self.all_fields().map(|ef| ef.vis()).collect()
    }

    /// Names of getters of all fields
    pub(crate) fn all_get_field_names(&self) -> Vec<&syn::Ident> {
        self.all_fields().map(|ef| ef.get_name()).collect()
    }

    /// Types of all fields (id and value).
    ///
    /// If this is an enum, empty vec.
    pub(crate) fn all_field_tys(&self) -> Vec<&syn::Type> {
        self.all_fields().map(|ef| ef.ty()).collect()
    }

    /// The name of "the struct" (this is the name the user gave, e.g., `Foo`).
    pub(crate) fn the_ident(&self) -> &syn::Ident {
        &self.struct_item.ident
    }

    /// Name of the struct the user gave plus:
    ///
    /// * its list of generic parameters
    /// * the generics "split for impl".
    pub(crate) fn the_ident_and_generics(
        &self,
    ) -> (
        &syn::Ident,
        &Punctuated<GenericParam, Comma>,
        ImplGenerics<'_>,
        TypeGenerics<'_>,
        Option<&WhereClause>,
    ) {
        let ident = &self.struct_item.ident;
        let (impl_generics, type_generics, where_clause) =
            self.struct_item.generics.split_for_impl();
        (
            ident,
            &self.struct_item.generics.params,
            impl_generics,
            type_generics,
            where_clause,
        )
    }

    /// Type of the jar for this struct
    pub(crate) fn jar_ty(&self) -> syn::Type {
        self.args.jar_ty()
    }

    /// checks if the "singleton" flag was set
    pub(crate) fn is_isingleton(&self) -> bool {
        self.args.singleton.is_some()
    }

    pub(crate) fn db_dyn_ty(&self) -> syn::Type {
        let jar_ty = self.jar_ty();
        let lt_db = self.maybe_elided_db_lifetime();
        parse_quote! {
            <#jar_ty as salsa::jar::Jar< #lt_db >>::DynDb
        }
    }

    /// Create "the struct" whose field is an id.
    /// This is the struct the user will refernece, but only if there
    /// are no lifetimes.
    pub(crate) fn the_struct_id(&self) -> syn::ItemStruct {
        assert_eq!(self.the_struct_kind(), TheStructKind::Id);

        let ident = self.the_ident();
        let visibility = &self.struct_item.vis;

        // Extract the attributes the user gave, but screen out derive, since we are adding our own,
        // and the customize attribute that we use for our own purposes.
        let attrs: Vec<_> = self
            .struct_item
            .attrs
            .iter()
            .filter(|attr| !attr.path.is_ident("derive"))
            .filter(|attr| !attr.path.is_ident("customize"))
            .collect();

        parse_quote_spanned! { ident.span() =>
            #(#attrs)*
            #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
            #visibility struct #ident(salsa::Id);
        }
    }

    /// Create the struct that the user will reference.
    /// If
    pub(crate) fn the_struct(&self, config_ident: &syn::Ident) -> syn::Result<syn::ItemStruct> {
        if self.struct_item.generics.params.is_empty() {
            Ok(self.the_struct_id())
        } else {
            let ident = self.the_ident();
            let visibility = &self.struct_item.vis;

            let generics = &self.struct_item.generics;
            if generics.params.len() != 1 || generics.lifetimes().count() != 1 {
                return Err(syn::Error::new_spanned(
                    &self.struct_item.generics,
                    "must have exactly one lifetime parameter",
                ));
            }

            let lifetime = generics.lifetimes().next().unwrap();

            // Extract the attributes the user gave, but screen out derive, since we are adding our own,
            // and the customize attribute that we use for our own purposes.
            let attrs: Vec<_> = self
                .struct_item
                .attrs
                .iter()
                .filter(|attr| !attr.path.is_ident("derive"))
                .filter(|attr| !attr.path.is_ident("customize"))
                .collect();

            Ok(parse_quote_spanned! { ident.span() =>
                #(#attrs)*
                #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
                #visibility struct #ident #generics (
                    *const salsa::tracked_struct::TrackedStructValue < #config_ident >,
                    std::marker::PhantomData < & #lifetime salsa::tracked_struct::TrackedStructValue < #config_ident > >
                );
            })
        }
    }

    /// Returns the visibility of this item
    pub(crate) fn visibility(&self) -> &syn::Visibility {
        &self.struct_item.vis
    }

    /// Returns the `constructor_name` in `Options` if it is `Some`, else `new`
    pub(crate) fn constructor_name(&self) -> syn::Ident {
        match self.args.constructor_name.clone() {
            Some(name) => name,
            None => Ident::new("new", self.the_ident().span()),
        }
    }

    /// Returns the lifetime to use for `'db`. This is normally whatever lifetime
    /// parameter the user put on the struct, but it might be a generated default
    /// if there is no such parameter. Using the name the user gave is important
    /// because it may appear in field types and the like.
    pub(crate) fn named_db_lifetime(&self) -> syn::Lifetime {
        match self.the_struct_kind() {
            TheStructKind::Id => self.default_db_lifetime(),
            TheStructKind::Pointer(db) => db,
        }
    }

    /// Returns lifetime to use for `'db`, substituting `'_` if there is no name required.
    /// This is convenient in function signatures where `'db` may not be in scope.
    pub(crate) fn maybe_elided_db_lifetime(&self) -> syn::Lifetime {
        match self.the_struct_kind() {
            TheStructKind::Id => syn::Lifetime {
                apostrophe: self.struct_item.ident.span(),
                ident: syn::Ident::new("_", self.struct_item.ident.span()),
            },
            TheStructKind::Pointer(db) => db,
        }
    }

    /// Normally we try to use whatever lifetime parameter the use gave us
    /// to represent `'db`; but if they didn't give us one, we need to use a default
    /// name. We choose `'db`.
    fn default_db_lifetime(&self) -> syn::Lifetime {
        let span = self.struct_item.ident.span();
        syn::Lifetime {
            apostrophe: span,
            ident: syn::Ident::new("db", span),
        }
    }

    /// Generate `impl salsa::AsId for Foo`
    pub(crate) fn as_id_impl(&self) -> Option<syn::ItemImpl> {
        match self.the_struct_kind() {
            TheStructKind::Id => {
                let ident = self.the_ident();
                let (impl_generics, type_generics, where_clause) =
                    self.struct_item.generics.split_for_impl();
                Some(parse_quote_spanned! { ident.span() =>
                    impl #impl_generics salsa::AsId for #ident #type_generics
                    #where_clause
                    {
                        fn as_id(self) -> salsa::Id {
                            self.0
                        }

                        fn from_id(id: salsa::Id) -> Self {
                            #ident(id)
                        }
                    }

                })
            }
            TheStructKind::Pointer(_) => None,
        }
    }

    /// Generate `impl salsa::DebugWithDb for Foo`, but only if this is an id struct.
    pub(crate) fn as_debug_with_db_impl(&self) -> Option<syn::ItemImpl> {
        if self.customizations.contains(&Customization::DebugWithDb) {
            return None;
        }

        let ident = self.the_ident();
        let (impl_generics, type_generics, where_clause) =
            self.struct_item.generics.split_for_impl();

        let db_type = self.db_dyn_ty();
        let ident_string = ident.to_string();

        // `::salsa::debug::helper::SalsaDebug` will use `DebugWithDb` or fallback to `Debug`
        let fields = self
            .all_fields()
            .map(|field| -> TokenStream {
                let field_name_string = field.name().to_string();
                let field_getter = field.get_name();
                let field_ty = ChangeLt::to_elided().in_type(field.ty());
                let db_type = ChangeLt::to_elided().in_type(&db_type);

                quote_spanned! { field.field.span() =>
                    debug_struct = debug_struct.field(
                        #field_name_string,
                        &::salsa::debug::helper::SalsaDebug::<#field_ty, #db_type>::salsa_debug(
                            #[allow(clippy::needless_borrow)]
                            &self.#field_getter(_db),
                            _db,
                        )
                    );
                }
            })
            .collect::<TokenStream>();

        // `use ::salsa::debug::helper::Fallback` is needed for the fallback to `Debug` impl
        Some(parse_quote_spanned! {ident.span()=>
            impl #impl_generics ::salsa::DebugWithDb<#db_type> for #ident #type_generics
            #where_clause
            {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>, _db: & #db_type) -> ::std::fmt::Result {
                    #[allow(unused_imports)]
                    use ::salsa::debug::helper::Fallback;
                    #[allow(unused_mut)]
                    let mut debug_struct = &mut f.debug_struct(#ident_string);
                    debug_struct = debug_struct.field("[salsa id]", &self.salsa_id().as_u32());
                    #fields
                    debug_struct.finish()
                }
            }
        })
    }

    /// Disallow `#[id]` attributes on the fields of this struct.
    ///
    /// If an `#[id]` field is found, return an error.
    ///
    /// # Parameters
    ///
    /// * `kind`, the attribute name (e.g., `input` or `interned`)
    pub(crate) fn disallow_id_fields(&self, kind: &str) -> syn::Result<()> {
        for ef in self.all_fields() {
            if ef.has_id_attr {
                return Err(syn::Error::new(
                    ef.name().span(),
                    format!("`#[id]` cannot be used with `#[salsa::{kind}]`"),
                ));
            }
        }

        Ok(())
    }
}

#[allow(clippy::type_complexity)]
pub(crate) const FIELD_OPTION_ATTRIBUTES: &[(&str, fn(&syn::Attribute, &mut SalsaField))] = &[
    ("id", |_, ef| ef.has_id_attr = true),
    ("return_ref", |_, ef| ef.has_ref_attr = true),
    ("no_eq", |_, ef| ef.has_no_eq_attr = true),
    ("get", |attr, ef| {
        ef.get_name = attr.parse_args().unwrap();
    }),
    ("set", |attr, ef| {
        ef.set_name = attr.parse_args().unwrap();
    }),
];

pub(crate) struct SalsaField {
    field: syn::Field,

    pub(crate) has_id_attr: bool,
    pub(crate) has_ref_attr: bool,
    pub(crate) has_no_eq_attr: bool,
    get_name: syn::Ident,
    set_name: syn::Ident,
}

impl SalsaField {
    pub(crate) fn new(field: &syn::Field) -> syn::Result<Self> {
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
            field: field.clone(),
            has_id_attr: false,
            has_ref_attr: false,
            has_no_eq_attr: false,
            get_name,
            set_name,
        };

        // Scan the attributes and look for the salsa attributes:
        for attr in &field.attrs {
            for (fa, func) in FIELD_OPTION_ATTRIBUTES {
                if attr.path.is_ident(fa) {
                    func(attr, &mut result);
                }
            }
        }

        Ok(result)
    }

    pub(crate) fn span(&self) -> Span {
        self.field.span()
    }

    /// The name of this field (all `SalsaField` instances are named).
    pub(crate) fn name(&self) -> &syn::Ident {
        self.field.ident.as_ref().unwrap()
    }

    /// The visibility of this field.
    pub(crate) fn vis(&self) -> &syn::Visibility {
        &self.field.vis
    }

    /// The type of this field (all `SalsaField` instances are named).
    pub(crate) fn ty(&self) -> &syn::Type {
        &self.field.ty
    }

    /// The name of this field's get method
    pub(crate) fn get_name(&self) -> &syn::Ident {
        &self.get_name
    }

    /// The name of this field's get method
    pub(crate) fn set_name(&self) -> &syn::Ident {
        &self.set_name
    }

    /// Do you clone the value of this field? (True if it is not a ref field)
    pub(crate) fn is_clone_field(&self) -> bool {
        !self.has_ref_attr
    }

    /// Do you potentially backdate the value of this field? (True if it is not a no-eq field)
    pub(crate) fn is_backdate_field(&self) -> bool {
        !self.has_no_eq_attr
    }
}
