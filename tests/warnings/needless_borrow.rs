#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Token {}

#[salsa::tracked]
struct TokenTree<'db> {
    #[returns(ref)]
    tokens: Vec<Token>,
}
