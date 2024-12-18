use crate::{
    db_lifetime,
    hygiene::Hygiene,
    options::Options,
    salsa_struct::{SalsaStruct, SalsaStructAllowedOptions},
};
use proc_macro2::TokenStream;

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn tracked_struct(
    args: proc_macro::TokenStream,
    struct_item: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&struct_item);
    let m = Macro {
        hygiene,
        args: syn::parse(args)?,
        struct_item,
    };
    m.try_macro()
}

type TrackedArgs = Options<TrackedStruct>;

struct TrackedStruct;

impl crate::options::AllowedOptions for TrackedStruct {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const NO_DEBUG: bool = true;

    const NO_CLONE: bool = false;

    const SINGLETON: bool = true;

    const DATA: bool = true;

    const DB: bool = false;

    const CYCLE_FN: bool = false;

    const CYCLE_INITIAL: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = true;
}

impl SalsaStructAllowedOptions for TrackedStruct {
    const KIND: &'static str = "tracked";

    const ALLOW_ID: bool = true;

    const HAS_LIFETIME: bool = true;

    const ALLOW_DEFAULT: bool = false;
}

struct Macro {
    hygiene: Hygiene,
    args: TrackedArgs,
    struct_item: syn::ItemStruct,
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
        let field_vis = salsa_struct.field_vis();
        let field_getter_ids = salsa_struct.field_getter_ids();
        let field_indices = salsa_struct.field_indices();
        let id_field_indices = salsa_struct.id_field_indices();
        let num_fields = salsa_struct.num_fields();
        let field_options = salsa_struct.field_options();
        let field_tys = salsa_struct.field_tys();
        let generate_debug_impl = salsa_struct.generate_debug_impl();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");
        let NonNull = self.hygiene.ident("NonNull");
        let Revision = self.hygiene.ident("Revision");

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                salsa::plumbing::setup_tracked_struct!(
                    attrs: [#(#attrs),*],
                    vis: #vis,
                    Struct: #struct_ident,
                    db_lt: #db_lt,
                    new_fn: #new_fn,
                    field_ids: [#(#field_ids),*],
                    field_getters: [#(#field_vis #field_getter_ids),*],
                    field_tys: [#(#field_tys),*],
                    field_indices: [#(#field_indices),*],
                    id_field_indices: [#(#id_field_indices),*],
                    field_options: [#(#field_options),*],
                    num_fields: #num_fields,
                    generate_debug_impl: #generate_debug_impl,
                    unused_names: [
                        #zalsa,
                        #zalsa_struct,
                        #Configuration,
                        #CACHE,
                        #Db,
                        #NonNull,
                        #Revision,
                    ]
                );
            },
        ))
    }
}
