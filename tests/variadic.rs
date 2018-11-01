use salsa::Database;

salsa::query_group! {
    trait HelloWorldDatabase: salsa::Database {
        fn input(a: u32, b: u32) -> u32 {
            type Input;
            storage input;
        }

        fn none() -> u32 {
            type None;
        }

        fn one(k: u32) -> u32 {
            type One;
        }

        fn two(a: u32, b: u32) -> u32 {
            type Two;
        }

        fn trailing(a: u32, b: u32,) -> u32 {
            type Trailing;
        }
    }
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

#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::Runtime<DatabaseStruct>,
}

impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseStruct> {
        &self.runtime
    }
}

salsa::database_storage! {
    struct DatabaseStorage for DatabaseStruct {
        impl HelloWorldDatabase {
            fn input() for Input;
            fn none() for None;
            fn one() for One;
            fn two() for Two;
            fn trailing() for Trailing;
        }
    }
}

#[test]
fn execute() {
    let mut db = DatabaseStruct::default();

    // test what happens with inputs:
    db.query_mut(Input).set((1, 2), 3);
    assert_eq!(db.input(1, 2), 3);

    assert_eq!(db.none(), 22);
    assert_eq!(db.one(11), 22);
    assert_eq!(db.two(11, 2), 22);
    assert_eq!(db.trailing(24, 2), 22);
}
