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
    configuration,
    options::{AllowedOptions, Options},
};
use heck::ToUpperCamelCase;
use proc_macro2::{Ident, Literal, Span, TokenStream};
use syn::spanned::Spanned;

pub(crate) enum SalsaStructKind {
    Input,
    Tracked,
    Interned,
}

pub(crate) struct SalsaStruct<A: AllowedOptions> {
    kind: SalsaStructKind,
    args: Options<A>,
    struct_item: syn::ItemStruct,
    fields: Vec<SalsaField>,
}

const BANNED_FIELD_NAMES: &[&str] = &["from", "new"];

impl<A: AllowedOptions> SalsaStruct<A> {
    pub(crate) fn new(
        kind: SalsaStructKind,
        args: proc_macro::TokenStream,
        input: proc_macro::TokenStream,
    ) -> syn::Result<Self> {
        let struct_item = syn::parse(input)?;
        Self::with_struct(kind, args, struct_item)
    }

    pub(crate) fn with_struct(
        kind: SalsaStructKind,
        args: proc_macro::TokenStream,
        struct_item: syn::ItemStruct,
    ) -> syn::Result<Self> {
        let args: Options<A> = syn::parse(args)?;
        let fields = Self::extract_options(&struct_item)?;
        Ok(Self {
            kind,
            args,
            struct_item,
            fields,
        })
    }

    /// Extract out the fields and their options:
    /// If this is a struct, it must use named fields, so we can define field accessors.
    /// If it is an enum, then this is not necessary.
    pub(crate) fn extract_options(struct_item: &syn::ItemStruct) -> syn::Result<Vec<SalsaField>> {
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

    pub(crate) fn is_identity_field(&self, field: &SalsaField) -> bool {
        match self.kind {
            SalsaStructKind::Input | SalsaStructKind::Tracked => field.has_id_attr,
            SalsaStructKind::Interned => true,
        }
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

    /// The name of the "identity" struct (this is the name the user gave, e.g., `Foo`).
    pub(crate) fn id_ident(&self) -> &syn::Ident {
        &self.struct_item.ident
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
        parse_quote! {
            <#jar_ty as salsa::jar::Jar<'_>>::DynDb
        }
    }

    /// The name of the "data" struct (this comes from the `data = Foo` option or,
    /// if that is not provided, by concatenating `Data` to the name of the struct).
    pub(crate) fn data_ident(&self) -> syn::Ident {
        match &self.args.data {
            Some(d) => d.clone(),
            None => syn::Ident::new(
                &format!("__{}Data", self.id_ident()),
                self.id_ident().span(),
            ),
        }
    }

    /// Generate `struct Foo(Id)`
    pub(crate) fn id_struct(&self) -> syn::ItemStruct {
        let ident = self.id_ident();
        let visibility = &self.struct_item.vis;

        // Extract the attributes the user gave, but screen out derive, since we are adding our own.
        let attrs: Vec<_> = self
            .struct_item
            .attrs
            .iter()
            .filter(|attr| !attr.path.is_ident("derive"))
            .collect();

        parse_quote! {
            #(#attrs)*
            #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
            #visibility struct #ident(salsa::Id);
        }
    }

    /// Generates the `struct FooData` struct (or enum).
    /// This type inherits all the attributes written by the user.
    ///
    /// When using named fields, we synthesize the struct and field names.
    ///
    /// When no named fields are available, copy the existing type.
    pub(crate) fn data_struct(&self) -> syn::ItemStruct {
        let ident = self.data_ident();
        let visibility = self.visibility();
        let all_field_names = self.all_field_names();
        let all_field_tys = self.all_field_tys();
        parse_quote! {
            /// Internal struct used for interned item
            #[derive(Eq, PartialEq, Hash, Clone)]
            #visibility struct #ident {
                #(
                    #all_field_names: #all_field_tys,
                )*
            }
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
            None => Ident::new("new", Span::call_site()),
        }
    }

    /// For each of the fields passed as an argument,
    /// generate a struct named `Ident_Field` and an impl
    /// of `salsa::function::Configuration` for that struct.
    pub(crate) fn field_config_structs_and_impls<'a>(
        &self,
        fields: impl Iterator<Item = &'a SalsaField>,
    ) -> (Vec<syn::ItemStruct>, Vec<syn::ItemImpl>) {
        let ident = &self.id_ident();
        let jar_ty = self.jar_ty();
        let visibility = self.visibility();
        fields
            .map(|ef| {
                let value_field_name = ef.name();
                let value_field_ty = ef.ty();
                let value_field_backdate = ef.is_backdate_field();
                let config_name = syn::Ident::new(
                    &format!(
                        "__{}",
                        format!("{}_{}", ident, value_field_name).to_upper_camel_case()
                    ),
                    value_field_name.span(),
                );
                let item_struct: syn::ItemStruct = parse_quote! {
                    #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
                    #visibility struct #config_name(std::convert::Infallible);
                };

                let execute_string = Literal::string(&format!("`execute` method for field `{}::{}` invoked",
                    ident,
                    ef.name(),
                ));

                let recover_from_cycle_string = Literal::string(&format!("`execute` method for field `{}::{}` invoked",
                    ident,
                    ef.name(),
                ));

                let should_backdate_value_fn = configuration::should_backdate_value_fn(value_field_backdate);
                let item_impl: syn::ItemImpl = parse_quote! {
                    impl salsa::function::Configuration for #config_name {
                        type Jar = #jar_ty;
                        type SalsaStruct = #ident;
                        type Key = #ident;
                        type Value = #value_field_ty;
                        const CYCLE_STRATEGY: salsa::cycle::CycleRecoveryStrategy = salsa::cycle::CycleRecoveryStrategy::Panic;

                        #should_backdate_value_fn

                        fn execute(db: &salsa::function::DynDb<Self>, key: Self::Key) -> Self::Value {
                            panic!(#execute_string)
                        }

                        fn recover_from_cycle(db: &salsa::function::DynDb<Self>, cycle: &salsa::Cycle, key: Self::Key) -> Self::Value {
                            panic!(#recover_from_cycle_string)
                        }
                    }
                };

                (item_struct, item_impl)
            })
            .unzip()
    }

    /// Generate `impl salsa::AsId for Foo`
    pub(crate) fn as_id_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();
        parse_quote! {
            impl salsa::AsId for #ident {
                fn as_id(self) -> salsa::Id {
                    self.0
                }

                fn from_id(id: salsa::Id) -> Self {
                    #ident(id)
                }
            }

        }
    }

    /// Generate `impl salsa::DebugWithDb for Foo`
    pub(crate) fn as_debug_with_db_impl(&self) -> syn::ItemImpl {
        let ident = self.id_ident();

        let db_type = self.db_dyn_ty();
        let ident_string = ident.to_string();

        // `::salsa::debug::helper::SalsaDebug` will use `DebugWithDb` or fallbak to `Debug`
        let fields = self
            .all_fields()
            .into_iter()
            .map(|field| -> TokenStream {
                let field_name_string = field.name().to_string();
                let field_getter = field.get_name();
                let field_ty = field.ty();

                let field_debug = quote_spanned! { field.field.span() =>
                    debug_struct = debug_struct.field(
                        #field_name_string,
                        &::salsa::debug::helper::SalsaDebug::<#field_ty, #db_type>::salsa_debug(
                            #[allow(clippy::needless_borrow)]
                            &self.#field_getter(_db),
                            _db,
                            _include_all_fields
                        )
                    );
                };

                if self.is_identity_field(field) {
                    quote_spanned! { field.field.span() =>
                        #field_debug
                    }
                } else {
                    quote_spanned! { field.field.span() =>
                        if _include_all_fields {
                            #field_debug
                        }
                    }
                }
            })
            .collect::<TokenStream>();

        // `use ::salsa::debug::helper::Fallback` is needed for the fallback to `Debug` impl
        parse_quote_spanned! {ident.span()=>
            impl ::salsa::DebugWithDb<#db_type> for #ident {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>, _db: &#db_type, _include_all_fields: bool) -> ::std::fmt::Result {
                    #[allow(unused_imports)]
                    use ::salsa::debug::helper::Fallback;
                    let mut debug_struct = &mut f.debug_struct(#ident_string);
                    debug_struct = debug_struct.field("[salsa id]", &self.0.as_u32());
                    #fields
                    debug_struct.finish()
                }
            }
        }
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
                &format!(
                    "the field name `{}` is disallowed in salsa structs",
                    field_name_str
                ),
            ));
        }

        let get_name = Ident::new(&field_name_str, Span::call_site());
        let set_name = Ident::new(&format!("set_{}", field_name_str), Span::call_site());
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
