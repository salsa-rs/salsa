use proc_macro2::TokenStream;

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
    let struct_item = parse_macro_input!(input as syn::ItemStruct);
    let m = Macro {
        hygiene,
        args,
        struct_item,
    };
    match m.try_macro() {
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

    const NON_SALSA_VALUES: bool = true;

    const SINGLETON: bool = false;

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

    const ALLOW_TRACKED: bool = false;

    const HAS_LIFETIME: bool = true;

    const ELIDABLE_LIFETIME: bool = true;

    const ALLOW_DEFAULT: bool = false;

    const ALLOW_MANUAL_RETENTION_PROOF: bool = true;

    const ALLOW_SELF_REF: bool = true;
}

struct Macro {
    hygiene: Hygiene,
    args: InternedArgs,
    struct_item: syn::ItemStruct,
}

impl Macro {
    #[allow(non_snake_case)]
    fn try_macro(&self) -> syn::Result<TokenStream> {
        let salsa_struct = SalsaStruct::new(&self.struct_item, &self.args)?;

        let attrs = &self.struct_item.attrs;
        let vis = &self.struct_item.vis;
        let struct_ident = &self.struct_item.ident;
        let struct_data_ident = format_ident!("{}Data", struct_ident);
        let db_lt = db_lifetime::db_lifetime(&self.struct_item.generics);
        let new_fn = salsa_struct.constructor_name();
        let num_fields = salsa_struct.num_fields();
        let generate_debug_impl = salsa_struct.generate_debug_impl();
        let has_lifetime = salsa_struct.generate_lifetime();
        let id = salsa_struct.id();
        let revisions = salsa_struct.revisions();

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
        let assembled_id = self.hygiene.ident("assembled_id");
        let assembled_data = self.hygiene.ident("assembled_data");
        let default_debug_fmt = self.hygiene.ident("default_debug_fmt");

        let mut identity_index = 0;
        let mut self_ref_index = 0;
        let fields = salsa_struct
            .fields_iter()
            .map(|(field_index, field)| {
                let partition_index = if field.has_self_ref_attr {
                    let index = self_ref_index;
                    self_ref_index += 1;
                    index
                } else {
                    let index = identity_index;
                    identity_index += 1;
                    index
                };
                (field_index, partition_index, field)
            })
            .collect::<Vec<_>>();

        let field_descriptors = fields.iter().map(|(field_index, partition_index, field)| {
            let field_id = field.field.ident.as_ref().unwrap();
            let field_ty = &field.field.ty;
            let field_vis = &field.field.vis;
            let field_getter_id = field.getter_name();
            let field_option = field.options();
            let field_self_ref = field.has_self_ref_attr;
            let indexed_ty = format_ident!("T{field_index}");
            let field_index = proc_macro2::Literal::usize_unsuffixed(*field_index);
            let partition_index = proc_macro2::Literal::usize_unsuffixed(*partition_index);
            let field_attrs = field.attrs();

            let (constructor_arg_ty, field_value) = if field_self_ref {
                (
                    quote!(::std::option::Option<#field_ty>),
                    quote!(#assembled_data.1.#partition_index.unwrap_or_else(|| {
                        let this: Self = #zalsa::FromId::from_id(#assembled_id);
                        this
                    })),
                )
            } else {
                (
                    quote!(#indexed_ty),
                    quote!(#zalsa::Lookup::into_owned(
                        #assembled_data.0.#partition_index
                    )),
                )
            };

            quote! {
                {
                    option: #field_option,
                    self_ref: #field_self_ref,
                    id: #field_id,
                    getter: #field_vis #field_getter_id,
                    ty: #field_ty,
                    index: #field_index,
                    constructor_arg: (#field_id: #constructor_arg_ty),
                    value: #field_value,
                    attrs: [#(#field_attrs),*]
                }
            }
        });

        let identity_field_descriptors = salsa_struct.non_self_ref_fields_iter().enumerate().map(
            |(key_index, (field_index, field))| {
                let field_id = field.field.ident.as_ref().unwrap();
                let field_ty = &field.field.ty;
                let indexed_ty = format_ident!("T{field_index}");
                let field_index = proc_macro2::Literal::usize_unsuffixed(field_index);
                let key_index = proc_macro2::Literal::usize_unsuffixed(key_index);
                quote! {
                    {
                        id: #field_id,
                        ty: #field_ty,
                        indexed_ty: #indexed_ty,
                        field_index: #field_index,
                        key_index: #key_index
                    }
                }
            },
        );

        let self_ref_field_descriptors = salsa_struct.self_ref_fields_iter().enumerate().map(
            |(key_index, (field_index, field))| {
                let field_id = field.field.ident.as_ref().unwrap();
                let field_ty = &field.field.ty;
                let field_index = proc_macro2::Literal::usize_unsuffixed(field_index);
                let key_index = proc_macro2::Literal::usize_unsuffixed(key_index);
                quote! {
                    {
                        id: #field_id,
                        ty: #field_ty,
                        field_index: #field_index,
                        key_index: #key_index
                    }
                }
            },
        );

        let self_type = if has_lifetime {
            syn::parse_quote!(#struct_ident<#db_lt>)
        } else {
            syn::parse_quote!(#struct_ident)
        };
        let assert_fields_are_salsa_values: TokenStream = salsa_struct
            .fields_iter()
            .map(|(_, field)| {
                let field_ty = &field.field.ty;
                let proof = field.manual_retention_proof.as_ref();
                if self.args.non_salsa_values.is_some() && proof.is_none() {
                    quote! {}
                } else {
                    crate::salsa_value::assert_salsa_value_field_with_proof(
                        &db_lt, &zalsa, field_ty, proof, &self_type,
                    )
                }
            })
            .collect();

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
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
                    fields: [#(#field_descriptors),*],
                    identity_fields: [#(#identity_field_descriptors),*],
                    self_ref_fields: [#(#self_ref_field_descriptors),*],
                    num_fields: #num_fields,
                    generate_debug_impl: #generate_debug_impl,
                    heap_size_fn: #(#heap_size_fn)*,
                    persist: #persist,
                    serialize_fn: #(#serialize_fn)*,
                    deserialize_fn: #(#deserialize_fn)*,
                    assert_fields_are_salsa_values: { #assert_fields_are_salsa_values },
                    unused_names: [
                        #zalsa,
                        #zalsa_struct,
                        #Configuration,
                        #CACHE,
                        #Db,
                        #assembled_id,
                        #assembled_data,
                        #default_debug_fmt,
                    ]
                );
            },
        ))
    }
}
