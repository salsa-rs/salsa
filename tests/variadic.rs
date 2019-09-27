#[salsa::query_group(HelloWorld)]
trait HelloWorldDatabase: salsa::Database {
    #[salsa::input]
    fn input(&self, a: u32, b: u32) -> u32;

    fn none(&self) -> u32;

    fn one(&self, k: u32) -> u32;

    fn two(&self, a: u32, b: u32) -> u32;

    fn trailing(&self, a: u32, b: u32) -> u32;
}

fn none(_db: &impl HelloWorldDatabase) -> u32 {
    22
}

fn one(_db: &impl HelloWorldDatabase, k: u32) -> u32 {
    k * 2
}

fn two(_db: &impl HelloWorldDatabase, a: u32, b: u32) -> u32 {
    a * b
}

fn trailing(_db: &impl HelloWorldDatabase, a: u32, b: u32) -> u32 {
    a - b
}

#[salsa::database(HelloWorld)]
#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}

#[test]
fn execute() {
    let mut db = DatabaseStruct::default();

    // test what happens with inputs:
    db.set_input(1, 2, 3);
    assert_eq!(db.input(1, 2), 3);

    assert_eq!(db.none(), 22);
    assert_eq!(db.one(11), 22);
    assert_eq!(db.two(11, 2), 22);
    assert_eq!(db.trailing(24, 2), 22);
}
