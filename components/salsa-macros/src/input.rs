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
        let field_ids = salsa_struct.field_ids();
        let field_indices = salsa_struct.field_indices();
        let num_fields = salsa_struct.num_fields();
        let field_setter_ids = salsa_struct.field_setter_ids();
        let field_options = salsa_struct.field_options();
        let field_tys = salsa_struct.field_tys();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                salsa::plumbing::setup_input_struct!(
                    // Attributes on the struct
                    attrs: [#(#attrs),*],

                    // Visibility of the struct
                    vis: #vis,

                    // Name of the struct
                    Struct: #struct_ident,

                    // Name user gave for `new`
                    new_fn: new, // FIXME

                    // A series of option tuples; see `setup_tracked_struct` macro
                    field_options: [#(#field_options),*],

                    // Field names
                    field_ids: [#(#field_ids),*],

                    // Names for field setter methods (typically `set_foo`)
                    field_setter_ids: [#(#field_setter_ids),*],

                    // Field types
                    field_tys: [#(#field_tys),*],

                    // Indices for each field from 0..N -- must be unsuffixed (e.g., `0`, `1`).
                    field_indices: [#(#field_indices),*],

                    // Number of fields
                    num_fields: #num_fields,

                    // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
                    // We have the procedural macro generate names for those items that are
                    // not used elsewhere in the user's code.
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
