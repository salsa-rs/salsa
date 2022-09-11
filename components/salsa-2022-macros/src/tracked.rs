use syn::{spanned::Spanned, Item};

pub(crate) fn tracked(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as Item);
    let res = match item {
        syn::Item::Struct(item) => crate::tracked_struct::tracked(args, item),
        syn::Item::Fn(item) => crate::tracked_fn::tracked_fn(args, item),
        syn::Item::Impl(item) => crate::tracked_fn::tracked_impl(args, item),
        _ => Err(syn::Error::new(
            item.span(),
            "tracked can only be applied to structs, functions, and impls",
        )),
    };
    match res {
        Ok(s) => s.into(),
        Err(err) => err.into_compile_error().into(),
    }
}
