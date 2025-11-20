use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

use crate::hygiene::Hygiene;
use crate::options::{AllowedOptions, AllowedPersistOptions, Options};
use crate::salsa_struct::{SalsaStruct, SalsaStructAllowedOptions};
use crate::{db_lifetime, token_stream_with_error};

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn interned(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(args as InternedArgs);
    let hygiene = Hygiene::from1(&input);
    let item = match syn::parse::<syn::Item>(input.clone()) {
        Ok(item) => item,
        Err(err) => return token_stream_with_error(input, err),
    };

    let lowered = match item {
        syn::Item::Struct(struct_item) => Ok(InternedInput::from_struct(struct_item)),
        syn::Item::Enum(enum_item) => InternedInput::from_enum(enum_item, &args),
        other => Err(syn::Error::new(
            other.span(),
            "interned can only be applied to structs and enums",
        )),
    };

    match lowered.and_then(|input| {
        Macro {
            hygiene,
            args,
            struct_item: input.struct_item,
            struct_data_ident: input.struct_data_ident,
            skip_conflict_rename: input.skip_conflict_rename,
            additional_items: input.additional_items,
        }
        .try_macro()
    }) {
        Ok(v) => v.into(),
        Err(e) => token_stream_with_error(input, e),
    }
}

type InternedArgs = Options<InternedStruct>;

struct InternedStruct;

impl AllowedOptions for InternedStruct {
    const RETURNS: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const DEBUG: bool = true;

    const NO_LIFETIME: bool = true;

    const NON_UPDATE_RETURN_TYPE: bool = false;

    const SINGLETON: bool = true;

    const DATA: bool = true;

    const DB: bool = false;

    const CYCLE_FN: bool = false;

    const CYCLE_INITIAL: bool = false;

    const CYCLE_RESULT: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = true;

    const ID: bool = true;

    const REVISIONS: bool = true;

    const HEAP_SIZE: bool = true;

    const SELF_TY: bool = false;

    const PERSIST: AllowedPersistOptions = AllowedPersistOptions::AllowedValue;
}

impl SalsaStructAllowedOptions for InternedStruct {
    const KIND: &'static str = "interned";

    const ALLOW_MAYBE_UPDATE: bool = false;

    const ALLOW_TRACKED: bool = false;

    const HAS_LIFETIME: bool = true;

    const ELIDABLE_LIFETIME: bool = true;

    const ALLOW_DEFAULT: bool = false;
}

struct Macro {
    hygiene: Hygiene,
    args: InternedArgs,
    struct_item: syn::ItemStruct,
    struct_data_ident: syn::Ident,
    skip_conflict_rename: bool,
    additional_items: Vec<TokenStream>,
}

impl Macro {
    #[allow(non_snake_case)]
    fn try_macro(&self) -> syn::Result<TokenStream> {
        let salsa_struct = SalsaStruct::new(&self.struct_item, &self.args)?;

        let attrs = &self.struct_item.attrs;
        let vis = &self.struct_item.vis;
        let struct_ident = &self.struct_item.ident;
        let db_lt = db_lifetime::db_lifetime(&self.struct_item.generics);
        let new_fn = salsa_struct.constructor_name();
        let field_ids = salsa_struct.field_ids();
        let field_indices = salsa_struct.field_indices();
        let num_fields = salsa_struct.num_fields();
        let field_vis = salsa_struct.field_vis();
        let field_getter_ids = salsa_struct.field_getter_ids();
        let field_options = salsa_struct.field_options();
        let field_tys = salsa_struct.field_tys();
        let field_indexed_tys = salsa_struct.field_indexed_tys();
        let field_unused_attrs = salsa_struct.field_attrs();
        let generate_debug_impl = salsa_struct.generate_debug_impl();
        let has_lifetime = salsa_struct.generate_lifetime();
        let id = salsa_struct.id();
        let revisions = salsa_struct.revisions();
        let mut struct_data_ident = self.struct_data_ident.clone();
        if !self.skip_conflict_rename
            && self.args.data.is_none()
            && struct_data_ident_conflicts(&struct_data_ident, &field_tys)
        {
            struct_data_ident = self.hygiene.scoped_ident(struct_ident, "Fields");
        }

        let (db_lt_arg, cfg, interior_lt) = if has_lifetime {
            (
                Some(db_lt.clone()),
                quote!(#struct_ident<'static>),
                db_lt.clone(),
            )
        } else {
            let span = syn::spanned::Spanned::span(&self.struct_item.generics);
            let static_lifetime = syn::Lifetime {
                apostrophe: span,
                ident: syn::Ident::new("static", span),
            };

            (None, quote!(#struct_ident), static_lifetime)
        };

        let persist = self.args.persist();
        let serialize_fn = salsa_struct.serialize_fn();
        let deserialize_fn = salsa_struct.deserialize_fn();

        let heap_size_fn = self.args.heap_size_fn.iter();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");
        let additional_items = &self.additional_items;

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                #(#additional_items)*
                salsa::plumbing::setup_interned_struct!(
                    attrs: [#(#attrs),*],
                    vis: #vis,
                    Struct: #struct_ident,
                    StructData: #struct_data_ident,
                    StructWithStatic: #cfg,
                    db_lt: #db_lt,
                    db_lt_arg: #db_lt_arg,
                    id: #id,
                    revisions: #(#revisions)*,
                    interior_lt: #interior_lt,
                    new_fn: #new_fn,
                    field_options: [#(#field_options),*],
                    field_ids: [#(#field_ids),*],
                    field_getters: [#(#field_vis #field_getter_ids),*],
                    field_tys: [#(#field_tys),*],
                    field_indices: [#(#field_indices),*],
                    field_indexed_tys: [#(#field_indexed_tys),*],
                    field_attrs: [#([#(#field_unused_attrs),*]),*],
                    num_fields: #num_fields,
                    generate_debug_impl: #generate_debug_impl,
                    heap_size_fn: #(#heap_size_fn)*,
                    persist: #persist,
                    serialize_fn: #(#serialize_fn)*,
                    deserialize_fn: #(#deserialize_fn)*,
                    unused_names: [
                        #zalsa,
                        #zalsa_struct,
                        #Configuration,
                        #CACHE,
                        #Db,
                    ]
                );
            },
        ))
    }
}

struct InternedInput {
    struct_item: syn::ItemStruct,
    struct_data_ident: syn::Ident,
    skip_conflict_rename: bool,
    additional_items: Vec<TokenStream>,
}

impl InternedInput {
    fn from_struct(struct_item: syn::ItemStruct) -> Self {
        let struct_data_ident = format_ident!("{}Data", struct_item.ident);
        Self {
            struct_item,
            struct_data_ident,
            skip_conflict_rename: false,
            additional_items: Vec::new(),
        }
    }

    fn from_enum(enum_item: syn::ItemEnum, args: &InternedArgs) -> syn::Result<Self> {
        let struct_ident = enum_item.ident.clone();
        if let Some(data_ident) = args.data.clone() {
            if data_ident == struct_ident {
                return Err(syn::Error::new(
                    data_ident.span(),
                    "data name conflicts with a generated identifier; please choose a different `data` name",
                ));
            }
        }

        let data_ident = args
            .data
            .clone()
            .unwrap_or_else(|| format_ident!("{}Data", struct_ident));

        let mut data_enum = enum_item;
        rename_type_idents(&mut data_enum, &struct_ident, &data_ident);
        data_enum.ident = data_ident.clone();

        let generics = data_enum.generics.clone();
        let (_, ty_generics, _) = generics.split_for_impl();
        let field_ty: syn::Type = parse_quote!(#data_ident #ty_generics);
        let struct_attrs = data_enum
            .attrs
            .iter()
            .filter(|attr| !attr.path().is_ident("derive"))
            .cloned()
            .collect::<Vec<_>>();
        let struct_vis = data_enum.vis.clone();
        let struct_item: syn::ItemStruct = parse_quote! {
            #(#struct_attrs)*
            #struct_vis struct #struct_ident #generics {
                value: #field_ty,
            }
        };

        Ok(Self {
            struct_item,
            // Use a distinct alias for the macro-internal tuple to avoid cycling with the data enum.
            struct_data_ident: format_ident!("{}Fields", struct_ident),
            // Allow conflict renaming to kick in if this identifier is already in use.
            skip_conflict_rename: false,
            additional_items: vec![data_enum.into_token_stream()],
        })
    }
}

fn rename_type_idents(enum_item: &mut syn::ItemEnum, from: &syn::Ident, to: &syn::Ident) {
    struct Renamer<'a> {
        from: &'a syn::Ident,
        to: &'a syn::Ident,
    }

    impl syn::visit_mut::VisitMut for Renamer<'_> {
        fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
            if node.qself.is_none()
                && node.path.leading_colon.is_none()
                && node.path.segments.len() == 1
                && node.path.segments.first().map(|s| &s.ident) == Some(self.from)
            {
                node.path.segments[0].ident = self.to.clone();
            }
            syn::visit_mut::visit_type_path_mut(self, node);
        }
    }

    let mut renamer = Renamer { from, to };
    renamer.visit_item_enum_mut(enum_item);
}

fn struct_data_ident_conflicts(ident: &syn::Ident, field_tys: &[&syn::Type]) -> bool {
    field_tys
        .iter()
        .copied()
        .any(|ty| type_contains_ident(ty, ident))
}

fn type_contains_ident(ty: &syn::Type, ident: &syn::Ident) -> bool {
    struct Finder<'a> {
        ident: &'a syn::Ident,
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for Finder<'_> {
        fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
            if node.qself.is_none()
                && node.path.leading_colon.is_none()
                && node.path.segments.len() == 1
                && node.path.segments.first().map(|s| &s.ident) == Some(self.ident)
            {
                self.found = true;
            }
            syn::visit::visit_type_path(self, node);
        }
    }

    let mut finder = Finder {
        ident,
        found: false,
    };

    finder.visit_type(ty);
    finder.found
}
