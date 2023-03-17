pub trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
pub struct Jar(SourceTree, SourceTree_all_items, use_tree);

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Item {}

#[salsa::tracked(jar = Jar)]
pub struct SourceTree {}

#[salsa::tracked(jar = Jar)]
impl SourceTree {
    #[salsa::tracked(return_ref)]
    pub fn all_items(self, _db: &dyn Db) -> Vec<Item> {
        todo!()
    }
}

#[salsa::tracked(jar = Jar, return_ref)]
fn use_tree(_db: &dyn Db, _tree: SourceTree) {}

#[allow(unused)]
fn use_it(db: &dyn Db, tree: SourceTree) {
    tree.all_items(db);
    use_tree(db, tree);
}
