pub trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
pub struct Jar(SourceTree<'_>, SourceTree_all_items, use_tree);

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Item {}

#[salsa::tracked]
pub struct SourceTree<'db> {}

#[salsa::tracked]
impl<'db> SourceTree<'db> {
    #[salsa::tracked(return_ref)]
    pub fn all_items(self, _db: &'db dyn Db) -> Vec<Item> {
        todo!()
    }
}

#[salsa::tracked(jar = Jar, return_ref)]
fn use_tree<'db>(_db: &'db dyn Db, _tree: SourceTree<'db>) {}

#[allow(unused)]
fn use_it(db: &dyn Db, tree: SourceTree) {
    tree.all_items(db);
    use_tree(db, tree);
}
