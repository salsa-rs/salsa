use proc_macro2::TokenStream;

use crate::hygiene::Hygiene;
use crate::options::{AllowedOptions, AllowedPersistOptions, InternedEvictionPolicy, Options};
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
    const EVICTION: bool = true;

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
        let field_ids = salsa_struct.field_ids();
        let field_indices = salsa_struct.field_indices();
        let num_fields = salsa_struct.num_fields();
        let field_vis = salsa_struct.field_vis();
        let field_getter_ids = salsa_struct.field_getter_ids();
        let field_options = salsa_struct.field_options();
        let field_tys = salsa_struct.field_tys();
        let field_manual_retention_proofs = salsa_struct.field_manual_retention_proofs();
        let field_indexed_tys = salsa_struct.field_indexed_tys();
        let field_unused_attrs = salsa_struct.field_attrs();
        let generate_debug_impl = salsa_struct.generate_debug_impl();
        let has_lifetime = salsa_struct.generate_lifetime();
        let id = salsa_struct.id();
        let legacy_revisions = salsa_struct.revisions().next();
        let eviction = self.args.eviction.as_ref();
        let nested_revisions = eviction.and_then(|eviction| eviction.revisions.as_ref());

        if let (Some(_), Some(nested_revisions)) = (legacy_revisions, nested_revisions) {
            return Err(syn::Error::new_spanned(
                nested_revisions,
                "`revisions` cannot be specified both inside and outside `eviction`",
            ));
        }

        let eviction_policy = eviction
            .map(|eviction| eviction.policy)
            .unwrap_or(InternedEvictionPolicy::Lru);
        if let (InternedEvictionPolicy::NoEviction, Some(revisions)) =
            (eviction_policy, legacy_revisions)
        {
            return Err(syn::Error::new_spanned(
                revisions,
                "the `no_eviction` policy cannot be combined with `revisions`",
            ));
        }

        let revisions = nested_revisions.or(legacy_revisions);
        let eviction_type = match eviction_policy {
            InternedEvictionPolicy::Lru => revisions.map_or_else(
                || quote!(::salsa::plumbing::interned::Lru),
                |revisions| {
                    quote!(
                        <::salsa::plumbing::interned::LruSelector<
                            { #revisions == ::core::usize::MAX }
                        > as ::salsa::plumbing::interned::SelectLru>::Eviction
                    )
                },
            ),
            InternedEvictionPolicy::NoEviction => {
                quote!(::salsa::plumbing::interned::NoopEviction)
            }
        };

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

        let assert_fields_are_salsa_values = if self.args.non_salsa_values.is_some() {
            quote! {}
        } else {
            field_tys
                .iter()
                .zip(field_manual_retention_proofs)
                .map(|(field_ty, has_manual_retention_proof)| {
                    crate::salsa_value::assert_salsa_value_field(
                        &db_lt,
                        &zalsa,
                        field_ty,
                        has_manual_retention_proof,
                    )
                })
                .collect()
        };

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
                    revisions: #revisions,
                    eviction: #eviction_type,
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
                    assert_fields_are_salsa_values: { #assert_fields_are_salsa_values },
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
