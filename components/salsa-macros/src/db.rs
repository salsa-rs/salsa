use proc_macro2::TokenStream;
use syn::{parse::Nothing, ItemStruct};

use crate::hygiene::Hygiene;

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
    let _nothing = syn::parse_macro_input!(args as Nothing);
    let db_macro = DbMacro {
        hygiene: Hygiene::from(&input),
        input: syn::parse_macro_input!(input as syn::ItemStruct),
    };
    match db_macro.try_db() {
        Ok(v) => v.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

struct DbMacro {
    hygiene: Hygiene,
    input: ItemStruct,
}

impl DbMacro {
    fn try_db(self) -> syn::Result<TokenStream> {
        let has_storage_impl = self.has_storage_impl()?;
        let input = self.input;
        Ok(quote! {
            #has_storage_impl
            #input
        })
    }

    fn find_storage_field(&self) -> syn::Result<syn::Ident> {
        let storage = "storage";
        for field in self.input.fields.iter() {
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
            &self.input.ident,
            "database struct must be a braced struct (`{}`) with a field named `storage`",
        ));
    }

    #[allow(non_snake_case)]
    fn has_storage_impl(&self) -> syn::Result<TokenStream> {
        let storage = self.find_storage_field()?;
        let db = &self.input.ident;

        let SalsaHasStorage = self.hygiene.ident("SalsaHasStorage");
        let SalsaStorage = self.hygiene.ident("SalsaStorage");

        Ok(quote! {
            const _: () = {
                use salsa::storage::HasStorage as #SalsaHasStorage;
                use salsa::storage::Storage as #SalsaStorage;

                unsafe impl #SalsaHasStorage for #db {
                    fn storage(&self) -> &#SalsaStorage<Self> {
                        &self.#storage
                    }

                    fn storage_mut(&mut self) -> &mut #SalsaStorage<Self> {
                        &mut self.#storage
                    }
                }
            };
        })
    }
}
