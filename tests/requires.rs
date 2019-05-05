//! Test that transparent (uncached) queries work


mod queries {
    #[salsa::query_group(InputGroupStorage)]
    pub trait InputGroup {
        #[salsa::input]
        fn input(&self, x: u32) -> u32;
    }

    #[salsa::query_group(PrivGroupAStorage)]
    pub trait PrivGroupA: InputGroup {
        fn private_a(&self, x: u32) -> u32;
    }

    fn private_a(db: &impl PrivGroupA, x: u32) -> u32{
        db.input(x)
    }

    #[salsa::query_group(PrivGroupBStorage)]
    pub trait PrivGroupB: InputGroup {
        fn private_b(&self, x: u32) -> u32;
    }

    fn private_b(db: &impl PrivGroupB, x: u32) -> u32{
        db.input(x)
    }

    #[salsa::query_group(PubGroupStorage, requires = "PrivGroupA + PrivGroupB")]
    pub trait PubGroup: InputGroup {
        fn public(&self, x: u32) -> u32;
    }


    fn public(db: &(impl PubGroup + PrivGroupA + PrivGroupB), x: u32) -> u32 {
        db.private_a(x) + db.private_b(x)
    }
}

#[salsa::database(
    queries::InputGroupStorage,
    queries::PrivGroupAStorage,
    queries::PrivGroupBStorage,
    queries::PubGroupStorage,
)]
#[derive(Default)]
struct Database {
    runtime: salsa::Runtime<Database>,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime<Database> {
        &self.runtime
    }
}

#[test]
fn require_clauses_work() {
    use queries::{InputGroup, PubGroup};
    let mut db = Database::default();

    db.set_input(1, 10);
    assert_eq!(db.public(1), 20);
}