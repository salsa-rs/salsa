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

use heck::CamelCase;

use crate::{configuration, options::Options};

pub(crate) struct SalsaStruct {
    args: Options<Self>,
    struct_item: syn::ItemStruct,
    fields: Vec<SalsaField>,
}

impl crate::options::AllowedOptions for SalsaStruct {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const JAR: bool = true;

    const DATA: bool = true;

    const DB: bool = false;
}

const BANNED_FIELD_NAMES: &[&str] = &["from", "new"];

impl SalsaStruct {
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
        let args = syn::parse(args)?;
        let fields = Self::extract_options(&struct_item)?;

        Ok(Self {
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

    /// Names of all fields (id and value).
    ///
    /// If this is an enum, empty vec.
    pub(crate) fn all_field_names(&self) -> Vec<&syn::Ident> {
        self.all_fields().map(|ef| ef.name()).collect()
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
                        format!("{}_{}", ident, value_field_name).to_camel_case()
                    ),
                    value_field_name.span(),
                );
                let item_struct: syn::ItemStruct = parse_quote! {
                    #[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
                    #visibility struct #config_name(std::convert::Infallible);
                };

                let should_backdate_value_fn = configuration::should_backdate_value_fn(value_field_backdate);
                let item_impl: syn::ItemImpl = parse_quote! {
                    impl salsa::function::Configuration for #config_name {
                        type Jar = #jar_ty;
                        type Key = #ident;
                        type Value = #value_field_ty;
                        const CYCLE_STRATEGY: salsa::cycle::CycleRecoveryStrategy = salsa::cycle::CycleRecoveryStrategy::Panic;

                        #should_backdate_value_fn

                        fn execute(db: &salsa::function::DynDb<Self>, key: Self::Key) -> Self::Value {
                            unreachable!()
                        }

                        fn recover_from_cycle(db: &salsa::function::DynDb<Self>, cycle: &salsa::Cycle, key: Self::Key) -> Self::Value {
                            unreachable!()
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

pub(crate) const FIELD_OPTION_ATTRIBUTES: &[(&str, fn(&syn::Attribute, &mut SalsaField))] = &[
    ("id", |_, ef| ef.has_id_attr = true),
    ("return_ref", |_, ef| ef.has_ref_attr = true),
    ("no_eq", |_, ef| ef.has_no_eq_attr = true),
];

pub(crate) struct SalsaField {
    field: syn::Field,

    pub(crate) has_id_attr: bool,
    pub(crate) has_ref_attr: bool,
    pub(crate) has_no_eq_attr: bool,
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

        let mut result = SalsaField {
            field: field.clone(),
            has_id_attr: false,
            has_ref_attr: false,
            has_no_eq_attr: false,
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

    /// The name of this field (all `EntityField` instances are named).
    pub(crate) fn name(&self) -> &syn::Ident {
        self.field.ident.as_ref().unwrap()
    }

    /// The type of this field (all `EntityField` instances are named).
    pub(crate) fn ty(&self) -> &syn::Type {
        &self.field.ty
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
