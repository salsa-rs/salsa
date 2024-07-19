#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Token {}

#[salsa::tracked]
struct TokenTree<'db> {
    #[return_ref]
    tokens: Vec<Token>,
}
