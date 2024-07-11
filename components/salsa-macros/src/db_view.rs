use proc_macro2::{Literal, TokenStream};
use syn::{spanned::Spanned, Token};

// Source:
//
// #[salsa::db_view]
// pub trait Db: salsa::DatabaseView<dyn Db> + ... {
//     ...
// }
pub(crate) fn db_view(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args: TokenStream = args.into();
    let input = syn::parse_macro_input!(input as syn::ItemTrait);
    match try_db_view(args, input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn try_db_view(args: TokenStream, input: syn::ItemTrait) -> syn::Result<TokenStream> {
    if let Some(token) = args.into_iter().next() {
        return Err(syn::Error::new_spanned(token, "unexpected token"));
    }

    // FIXME: check for `salsa::DataviewView<dyn Db>` supertrait?

    let view_impl = view_impl(&input);

    Ok(quote! {
        #input
        #view_impl
    })
}

#[allow(non_snake_case)]
fn view_impl(input: &syn::ItemTrait) -> syn::Item {
    let DB = syn::Ident::new("_DB", proc_macro2::Span::call_site());
    let Database = syn::Ident::new("_Database", proc_macro2::Span::call_site());
    let DatabaseView = syn::Ident::new("_DatabaseView", proc_macro2::Span::call_site());
    let upcasts = syn::Ident::new("_upcasts", proc_macro2::Span::call_site());
    let UserTrait = &input.ident;

    parse_quote! {
        const _: () = {
            use salsa::DatabaseView as #DatabaseView;
            use salsa::Database as #Database;

            impl<#DB: #Database> #DatabaseView<dyn #UserTrait> for #DB {
                fn add_view_to_db(&self) {
                    let #upcasts = self.upcasts_for_self();
                    #upcasts.add::<dyn #UserTrait>(|t| t, |t| t);
                }
            }
        };
    }
}

pub struct Args {}

impl syn::parse::Parse for Args {
    fn parse(_input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {})
    }
}
