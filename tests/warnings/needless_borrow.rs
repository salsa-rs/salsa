trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
struct Jar(TokenTree<'_>);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Token {}

impl salsa::DebugWithDb<dyn Db + '_> for Token {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>, _db: &dyn Db) -> std::fmt::Result {
        unreachable!()
    }
}

#[salsa::tracked]
struct TokenTree<'db> {
    #[return_ref]
    tokens: Vec<Token>,
}
