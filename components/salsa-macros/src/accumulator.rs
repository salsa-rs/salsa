use proc_macro2::TokenStream;

use crate::{
    hygiene::Hygiene,
    options::{AllowedOptions, Options},
};

// #[salsa::accumulator(jar = Jar0)]
// struct Accumulator(DataType);

pub(crate) fn accumulator(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let hygiene = Hygiene::from1(&input);
    let args = syn::parse_macro_input!(args as Options<Accumulator>);
    let struct_item = syn::parse_macro_input!(input as syn::ItemStruct);
    let ident = struct_item.ident.clone();
    let m = StructMacro {
        hygiene,
        args,
        struct_item,
    };
    match m.try_expand() {
        Ok(v) => crate::debug::dump_tokens(ident, v).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

struct Accumulator;

impl AllowedOptions for Accumulator {
    const RETURN_REF: bool = false;
    const SPECIFY: bool = false;
    const NO_EQ: bool = false;
    const NO_DEBUG: bool = true;
    const NO_CLONE: bool = true;
    const SINGLETON: bool = false;
    const DATA: bool = false;
    const DB: bool = false;
    const RECOVERY_FN: bool = false;
    const LRU: bool = false;
    const CONSTRUCTOR_NAME: bool = false;
}

struct StructMacro {
    hygiene: Hygiene,
    args: Options<Accumulator>,
    struct_item: syn::ItemStruct,
}

#[allow(non_snake_case)]
impl StructMacro {
    fn try_expand(self) -> syn::Result<TokenStream> {
        let ident = self.struct_item.ident.clone();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let CACHE = self.hygiene.ident("CACHE");
        let ingredient = self.hygiene.ident("ingredient");

        let struct_item = self.struct_item;

        let mut derives = vec![];
        if self.args.no_debug.is_none() {
            derives.push(quote!(Debug));
        }
        if self.args.no_clone.is_none() {
            derives.push(quote!(Clone));
        }

        Ok(quote! {
            #[derive(#(#derives),*)]
            #struct_item

            salsa::plumbing::setup_accumulator_impl! {
                Struct: #ident,
                unused_names: [
                    #zalsa,
                    #zalsa_struct,
                    #CACHE,
                    #ingredient,
                ]
            }
        })
    }
}
