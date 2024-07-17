use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::Nothing;

use crate::{
    hygiene::Hygiene,
    options::{AllowedOptions, Options},
    tracked_fn::TrackedFn,
};

pub(crate) fn tracked_impl(
    args: proc_macro::TokenStream,
    item: syn::ItemImpl,
) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&item);
    let _: Nothing = syn::parse(args)?;
    let m = Macro { hygiene };
    m.try_generate(item)
}

struct Macro {
    hygiene: Hygiene,
}

impl Macro {
    fn try_generate(&self, mut impl_item: syn::ItemImpl) -> syn::Result<TokenStream> {
        let mut member_items = std::mem::replace(&mut impl_item.items, vec![]);
        for member_item in &mut member_items {
            self.modify_member(&impl_item, member_item)?;
        }
        Ok(impl_item.into_token_stream())
    }

    fn modify_member(
        &self,
        _impl_item: &syn::ItemImpl,
        member_item: &mut syn::ImplItem,
    ) -> syn::Result<()> {
        let syn::ImplItem::Fn(fn_item) = member_item else {
            return Ok(());
        };

        let Some(tracked_attr) = fn_item.attrs.iter().find(|a| self.is_tracked_attr(a)) else {
            return Ok(());
        };

        let _options: Options<TrackedFn> = tracked_attr.parse_args()?;

        todo!()
    }

    fn is_tracked_attr(&self, attr: &syn::Attribute) -> bool {
        if attr.path().segments.len() != 2 {
            return false;
        }

        let seg0 = &attr.path().segments[0];
        let seg1 = &attr.path().segments[0];

        seg0.ident == "salsa"
            && seg1.ident == "tracked"
            && seg0.arguments.is_empty()
            && seg1.arguments.is_empty()
    }
}
