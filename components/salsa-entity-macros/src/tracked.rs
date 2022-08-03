use syn::{spanned::Spanned, Item};

pub(crate) fn tracked(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as Item);
    match item {
        syn::Item::Struct(item) => crate::tracked_struct::tracked(args, item),
        syn::Item::Fn(item) => crate::tracked_fn::tracked(args, item),
        _ => syn::Error::new(
            item.span(),
            &format!("tracked can be applied to structs and functions only"),
        )
        .into_compile_error()
        .into(),
    }
}
