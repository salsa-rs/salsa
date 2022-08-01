pub(crate) enum DataItem {
    Struct(syn::ItemStruct),
    Enum(syn::ItemEnum),
}

impl syn::parse::Parse for DataItem {
    fn parse(input: &syn::parse::ParseBuffer<'_>) -> Result<Self, syn::Error> {
        match syn::Item::parse(input)? {
            syn::Item::Enum(item) => Ok(DataItem::Enum(item)),
            syn::Item::Struct(item) => Ok(DataItem::Struct(item)),
            _ => Err(input.error("expected an enum or a struct")),
        }
    }
}

impl DataItem {
    pub(crate) fn attrs(&self) -> &[syn::Attribute] {
        match self {
            DataItem::Struct(s) => &s.attrs,
            DataItem::Enum(e) => &e.attrs,
        }
    }

    /// Returns the name of this struct/enum.
    pub(crate) fn ident(&self) -> &syn::Ident {
        match self {
            DataItem::Struct(s) => &s.ident,
            DataItem::Enum(e) => &e.ident,
        }
    }

    /// Returns a new version of this struct/enum but with the given name `ident`.
    pub(crate) fn with_ident(&self, ident: syn::Ident) -> DataItem {
        match self {
            DataItem::Struct(s) => {
                let mut s = s.clone();
                s.ident = ident;
                DataItem::Struct(s)
            }
            DataItem::Enum(s) => {
                let mut s = s.clone();
                s.ident = ident;
                DataItem::Enum(s)
            }
        }
    }

    /// Returns the visibility of this struct/enum.
    pub(crate) fn visibility(&self) -> &syn::Visibility {
        match self {
            DataItem::Struct(s) => &s.vis,
            DataItem::Enum(e) => &e.vis,
        }
    }

    /// If this is a struct, returns the list of fields.
    ///
    /// If this is an enum, returns None.
    pub(crate) fn fields(&self) -> Option<&syn::Fields> {
        match self {
            DataItem::Struct(s) => Some(&s.fields),
            DataItem::Enum(_) => None,
        }
    }
}

impl quote::ToTokens for DataItem {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            DataItem::Struct(s) => s.to_tokens(tokens),
            DataItem::Enum(e) => e.to_tokens(tokens),
        }
    }
}
