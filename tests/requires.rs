//! Test `salsa::requires` attribute for private query dependencies
//! https://github.com/salsa-rs/salsa-rfcs/pull/3

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

    fn private_a(db: &mut impl PrivGroupA, x: u32) -> u32 {
        db.input(x)
    }

    #[salsa::query_group(PrivGroupBStorage)]
    pub trait PrivGroupB: InputGroup {
        fn private_b(&self, x: u32) -> u32;
    }

    fn private_b(db: &mut impl PrivGroupB, x: u32) -> u32 {
        db.input(x)
    }

    #[salsa::query_group(PubGroupStorage)]
    #[salsa::requires(PrivGroupA)]
    #[salsa::requires(PrivGroupB)]
    pub trait PubGroup: InputGroup {
        fn public(&self, x: u32) -> u32;
    }

    fn public(db: &mut (impl PubGroup + PrivGroupA + PrivGroupB), x: u32) -> u32 {
        db.private_a(x) + db.private_b(x)
    }
}

#[salsa::database(
    queries::InputGroupStorage,
    queries::PrivGroupAStorage,
    queries::PrivGroupBStorage,
    queries::PubGroupStorage
)]
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
fn require_clauses_work() {
    use queries::{InputGroupMut, PubGroup};
    let mut db = Database::default();

    db.set_input(1, 10);
    assert_eq!(db.public(1), 20);
}
