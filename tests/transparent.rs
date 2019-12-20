//! Test that transparent (uncached) queries work

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup {
    #[salsa::input]
    fn input(&self, x: u32) -> u32;
    #[salsa::transparent]
    fn wrap(&self, x: u32) -> u32;
    fn get(&self, x: u32) -> u32;
}

fn wrap(db: &mut impl QueryGroup, x: u32) -> u32 {
    db.input(x)
}

fn get(db: &mut impl QueryGroup, x: u32) -> u32 {
    db.wrap(x)
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    runtime: salsa::Runtime<Database>,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}

#[test]
fn transparent_queries_work() {
    let mut db = Database::default();

    db.set_input(1, 10);
    assert_eq!(db.get(1), 10);
    assert_eq!(db.get(1), 10);

    db.set_input(1, 92);
    assert_eq!(db.get(1), 92);
    assert_eq!(db.get(1), 92);
}
