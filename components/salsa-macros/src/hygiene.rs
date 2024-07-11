use std::collections::HashSet;

pub struct Hygiene {
    user_tokens: HashSet<String>,
}

impl From<&proc_macro::TokenStream> for Hygiene {
    fn from(input: &proc_macro::TokenStream) -> Self {
        let mut user_tokens = HashSet::new();
        push_idents(input.clone(), &mut user_tokens);
        Self { user_tokens }
    }
}

fn push_idents(input: proc_macro::TokenStream, user_tokens: &mut HashSet<String>) {
    input.into_iter().for_each(|token| match token {
        proc_macro::TokenTree::Group(g) => {
            push_idents(g.stream(), user_tokens);
        }
        proc_macro::TokenTree::Ident(ident) => {
            user_tokens.insert(ident.to_string());
        }
        proc_macro::TokenTree::Punct(_) => (),
        proc_macro::TokenTree::Literal(_) => (),
    })
}

impl Hygiene {
    /// Generates an identifier similar to `text` but
    /// distinct from any identifiers that appear in the user's
    /// code.
    pub(crate) fn ident(&self, text: &str) -> syn::Ident {
        let mut buffer = String::from(text);

        while self.user_tokens.contains(&buffer) {
            buffer.push('_');
        }

        syn::Ident::new(&buffer, proc_macro2::Span::call_site())
    }
}
