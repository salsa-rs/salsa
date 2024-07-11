use proc_macro2::{Literal, TokenStream};
use syn::{parse::Nothing, spanned::Spanned, Token};

// Source:
//
// #[salsa::db(Jar0, Jar1, Jar2)]
// pub struct Database {
//    storage: salsa::Storage<Self>,
// }

pub(crate) fn db(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(args as Args);
    let input = syn::parse_macro_input!(input as syn::ItemStruct);
    match args.try_db(&input) {
        Ok(v) => quote! { #input #v }.into(),
        Err(e) => {
            let error = e.to_compile_error();
            quote! { #input #error }.into()
        }
    }
}

pub struct Args {}

impl syn::parse::Parse for Args {
    fn parse(_input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        Ok(Args {})
    }
}

impl Args {
    fn try_db(self, input: &syn::ItemStruct) -> syn::Result<TokenStream> {
        let storage = self.find_storage_field(input)?;

        Ok(quote! {
            #input
        })
    }

    fn find_storage_field(&self, input: &syn::ItemStruct) -> syn::Result<syn::Ident> {
        let storage = "storage";
        for field in input.fields.iter() {
            if let Some(i) = &field.ident {
                if i == storage {
                    return Ok(i.clone());
                }
            } else {
                return Err(syn::Error::new_spanned(
                    field,
                    "database struct must be a braced struct (`{}`) with a field named `storage`",
                ));
            }
        }

        return Err(syn::Error::new_spanned(
            &input.ident,
            "database struct must be a braced struct (`{}`) with a field named `storage`",
        ));
    }
}
