use proc_macro2::TokenStream;
use syn::{parse::Nothing, spanned::Spanned};

use crate::hygiene::Hygiene;

// #[salsa::accumulator(jar = Jar0)]
// struct Accumulator(DataType);

pub(crate) fn accumulator(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let hygiene = Hygiene::from1(&input);
    let _ = syn::parse_macro_input!(args as Nothing);
    let struct_item = syn::parse_macro_input!(input as syn::ItemStruct);
    let ident = struct_item.ident.clone();
    let m = StructMacro {
        hygiene,
        struct_item,
    };
    match m.try_expand() {
        Ok(v) => crate::debug::dump_tokens(&ident, v).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

struct StructMacro {
    hygiene: Hygiene,
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

        Ok(quote! {
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
