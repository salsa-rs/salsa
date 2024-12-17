use crate::{
    db_lifetime,
    hygiene::Hygiene,
    options::Options,
    salsa_struct::{SalsaStruct, SalsaStructAllowedOptions},
    token_stream_with_error,
};
use proc_macro2::TokenStream;

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn interned_sans_lifetime(
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

#[derive(Debug)]
struct InternedStruct;

impl crate::options::AllowedOptions for InternedStruct {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const NO_DEBUG: bool = true;

    const NO_CLONE: bool = false;

    const SINGLETON: bool = true;

    const DATA: bool = true;

    const DB: bool = false;

    const RECOVERY_FN: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = true;

    const ID: bool = true;
}

impl SalsaStructAllowedOptions for InternedStruct {
    const KIND: &'static str = "interned";

    const ALLOW_ID: bool = false;

    const HAS_LIFETIME: bool = false;

    const ALLOW_DEFAULT: bool = false;
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
        let generate_debug_impl = salsa_struct.generate_debug_impl();
        let id = salsa_struct.id();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                salsa::plumbing::setup_interned_struct_sans_lifetime!(
                    attrs: [#(#attrs),*],
                    vis: #vis,
                    Struct: #struct_ident,
                    db_lt: #db_lt,
                    id: #id,
                    new_fn: #new_fn,
                    field_options: [#(#field_options),*],
                    field_ids: [#(#field_ids),*],
                    field_getters: [#(#field_vis #field_getter_ids),*],
                    field_tys: [#(#field_tys),*],
                    field_indices: [#(#field_indices),*],
                    field_indexed_tys: [#(#field_indexed_tys),*],
                    num_fields: #num_fields,
                    generate_debug_impl: #generate_debug_impl,
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
