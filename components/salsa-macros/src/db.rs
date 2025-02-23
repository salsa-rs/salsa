use proc_macro2::TokenStream;
use syn::parse::Nothing;

use crate::{hygiene::Hygiene, token_stream_with_error};

// Source:
//
// #[salsa::db]
// pub struct Database {
//    storage: salsa::Storage<Self>,
// }

pub(crate) fn db(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let _nothing = syn::parse_macro_input!(args as Nothing);
    let hygiene = Hygiene::from1(&input);
    let item = parse_macro_input!(input as syn::Item);
    let db_macro = DbMacro { hygiene };
    match db_macro.try_db(item) {
        Ok(v) => crate::debug::dump_tokens("db", v).into(),
        Err(e) => token_stream_with_error(input, e),
    }
}

struct DbMacro {
    hygiene: Hygiene,
}

#[allow(non_snake_case)]
impl DbMacro {
    fn try_db(self, input: syn::Item) -> syn::Result<TokenStream> {
        match input {
            syn::Item::Struct(input) => {
                let has_storage_impl = self.has_storage_impl(&input)?;
                Ok(quote! {
                    #has_storage_impl
                    #input
                })
            }
            syn::Item::Trait(mut input) => {
                self.add_salsa_view_method(&mut input)?;
                Ok(quote! {
                    #input
                })
            }
            syn::Item::Impl(mut input) => {
                self.add_salsa_view_method_impl(&mut input)?;
                Ok(quote! {
                    #input
                })
            }
            _ => Err(syn::Error::new_spanned(
                input,
                "`db` must be applied to a struct, trait, or impl",
            )),
        }
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

        Err(syn::Error::new_spanned(
            &input.ident,
            "database struct must be a braced struct (`{}`) with a field named `storage`",
        ))
    }

    fn has_storage_impl(&self, input: &syn::ItemStruct) -> syn::Result<TokenStream> {
        let storage = self.find_storage_field(input)?;
        let db = &input.ident;
        let zalsa = self.hygiene.ident("zalsa");

        Ok(quote! {
            const _: () = {
                use salsa::plumbing as #zalsa;

                unsafe impl #zalsa::HasStorage for #db {
                    fn storage(&self) -> &#zalsa::Storage<Self> {
                        &self.#storage
                    }

                    fn storage_mut(&mut self) -> &mut #zalsa::Storage<Self> {
                        &mut self.#storage
                    }
                }
            };
        })
    }

    fn add_salsa_view_method(&self, input: &mut syn::ItemTrait) -> syn::Result<()> {
        let trait_name = &input.ident;
        input.items.push(parse_quote! {
            #[doc(hidden)]
            fn zalsa_register_downcaster(&self);
        });

        let comment = format!(" Downcast a [`dyn Database`] to a [`dyn {trait_name}`]");
        input.items.push(parse_quote! {
            #[doc = #comment]
            ///
            /// # Safety
            ///
            /// The input database must be of type `Self`.
            #[doc(hidden)]
            unsafe fn downcast(db: &dyn salsa::plumbing::Database) -> &dyn #trait_name where Self: Sized;
        });
        Ok(())
    }

    fn add_salsa_view_method_impl(&self, input: &mut syn::ItemImpl) -> syn::Result<()> {
        let Some((_, TraitPath, _)) = &input.trait_ else {
            return Err(syn::Error::new_spanned(
                &input.self_ty,
                "impl must be on a trait",
            ));
        };

        input.items.push(parse_quote! {
            #[doc(hidden)]
            #[inline(always)]
            fn zalsa_register_downcaster(&self)  {
                salsa::plumbing::views(self).add(<Self as #TraitPath>::downcast);
            }
        });
        input.items.push(parse_quote! {
            #[doc(hidden)]
            unsafe fn downcast(db: &dyn salsa::plumbing::Database) -> &dyn #TraitPath where Self: Sized {
                debug_assert_eq!(db.type_id(), ::core::any::TypeId::of::<Self>());
                // SAFETY: Same as the safety of the `downcast` method.
                unsafe { &*salsa::plumbing::transmute_data_ptr::<dyn salsa::plumbing::Database, Self>(db) }
            }
        });
        Ok(())
    }
}
