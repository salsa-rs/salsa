#![cfg(test)]

pub struct DatabaseImpl<'a> {
    runtime: salsa::Runtime<DatabaseImpl<'a>>,
    input: &'a str,
}

impl<'a> DatabaseImpl<'a> {
    pub fn new(input: &'a str) -> DatabaseImpl<'a> {
        DatabaseImpl {
            runtime: Default::default(),
            input,
        }
    }
}

pub trait DatabaseWithInput<'a>: salsa::Database {
    fn input(&self) -> &'a str;
}

impl<'a> salsa::Database for DatabaseImpl<'a> {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl<'a>> {
        &self.runtime
    }
}

impl<'a> DatabaseWithInput<'a> for DatabaseImpl<'a> {
    fn input(&self) -> &'a str {
        self.input
    }
}

salsa::database_storage! {
    pub struct DatabaseImplStorage<'a> for DatabaseImpl<'a> {
        impl Database<'a> {
            fn unmodified() for Unmodified<'a>;
            fn uppercase() for Uppercase<'a>;
        }
    }
}

salsa::query_group! {
    trait Database<'a>: DatabaseWithInput<'a> {
        fn unmodified() -> &'a str {
            type Unmodified;
            storage volatile;
        }

        fn uppercase() -> String {
            type Uppercase;
        }
    }
}

fn unmodified<'a>(db: &impl Database<'a>) -> &'a str {
    db.input()
}

fn uppercase<'a>(db: &impl Database<'a>) -> String {
    db.unmodified().to_uppercase()
}

#[test]
fn static_ref() {
    let input: &'static str = "Hello Salsa";
    let db = DatabaseImpl::new(input);
    assert_eq!(db.unmodified(), "Hello Salsa");
    assert_eq!(db.uppercase(), "HELLO SALSA");
}

#[test]
fn local_ref() {
    let input = String::from("Hello Salsa");
    let db = DatabaseImpl::new(&input);
    assert_eq!(db.unmodified(), "Hello Salsa");
    assert_eq!(db.uppercase(), "HELLO SALSA");
}
