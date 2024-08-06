use crate::{
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
pub(crate) fn input(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(args as InputArgs);
    let hygiene = Hygiene::from1(&input);
    let struct_item = syn::parse_macro_input!(input as syn::ItemStruct);
    let m = Macro {
        hygiene,
        args,
        struct_item,
    };
    match m.try_macro() {
        Ok(v) => v.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

type InputArgs = Options<InputStruct>;

struct InputStruct;

impl crate::options::AllowedOptions for InputStruct {
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
}

impl SalsaStructAllowedOptions for InputStruct {
    const KIND: &'static str = "input";

    const ALLOW_ID: bool = false;

    const HAS_LIFETIME: bool = false;

    const ALLOW_DEFAULT: bool = true;
}

struct Macro {
    hygiene: Hygiene,
    args: InputArgs,
    struct_item: syn::ItemStruct,
}

impl Macro {
    #[allow(non_snake_case)]
    fn try_macro(&self) -> syn::Result<TokenStream> {
        let salsa_struct = SalsaStruct::new(&self.struct_item, &self.args)?;

        let attrs = &self.struct_item.attrs;
        let vis = &self.struct_item.vis;
        let struct_ident = &self.struct_item.ident;
        let new_fn = salsa_struct.constructor_name();
        let field_ids = salsa_struct.field_ids();
        let field_indices = salsa_struct.field_indices();
        let num_fields = salsa_struct.num_fields();
        let field_vis = salsa_struct.field_vis();
        let field_getter_ids = salsa_struct.field_getter_ids();
        let field_setter_ids = salsa_struct.field_setter_ids();
        let required_fields = salsa_struct.required_fields();
        let field_options = salsa_struct.field_options();
        let field_tys = salsa_struct.field_tys();
        let field_durability_ids = salsa_struct.field_durability_ids();
        let is_singleton = self.args.singleton.is_some();
        let generate_debug_impl = salsa_struct.generate_debug_impl();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let Builder = self.hygiene.ident("Builder");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                salsa::plumbing::setup_input_struct!(
                    attrs: [#(#attrs),*],
                    vis: #vis,
                    Struct: #struct_ident,
                    new_fn: #new_fn,
                    field_options: [#(#field_options),*],
                    field_ids: [#(#field_ids),*],
                    field_getters: [#(#field_vis #field_getter_ids),*],
                    field_setters: [#(#field_vis #field_setter_ids),*],
                    field_tys: [#(#field_tys),*],
                    field_indices: [#(#field_indices),*],
                    required_fields: [#(#required_fields),*],
                    field_durability_ids: [#(#field_durability_ids),*],
                    num_fields: #num_fields,
                    is_singleton: #is_singleton,
                    generate_debug_impl: #generate_debug_impl,
                    unused_names: [
                        #zalsa,
                        #zalsa_struct,
                        #Configuration,
                        #Builder,
                        #CACHE,
                        #Db,
                    ]
                );
            },
        ))
    }
}
