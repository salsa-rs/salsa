use salsa::Database;

salsa::query_group! {
    trait HelloWorldDatabase: salsa::Database {
        fn input(key: ()) -> String {
            type Input;
            storage input;
        }

        fn length(key: ()) -> usize {
            type Length;
        }

        fn double_length(key: ()) -> usize {
            type DoubleLength;
        }
    }
}

fn length(db: &impl HelloWorldDatabase, (): ()) -> usize {
    let l = db.input(()).len();
    assert!(l > 0); // not meant to be invoked with no input
    l
}

fn double_length(db: &impl HelloWorldDatabase, (): ()) -> usize {
    db.length(()) * 2
}

#[derive(Default)]
struct DatabaseStruct {
    runtime: salsa::runtime::Runtime<DatabaseStruct>,
}

impl salsa::Database for DatabaseStruct {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<DatabaseStruct> {
        &self.runtime
    }
}

salsa::database_storage! {
    struct DatabaseStorage for DatabaseStruct {
        impl HelloWorldDatabase {
            fn input() for Input;
            fn length() for Length;
            fn double_length() for DoubleLength;
        }
    }
}

#[test]
fn normal() {
    let db = DatabaseStruct::default();
    db.query(Input).set((), format!("Hello, world"));
    assert_eq!(db.double_length(()), 24);
    db.query(Input).set((), format!("Hello, world!"));
    assert_eq!(db.double_length(()), 26);
}

#[test]
#[should_panic]
fn use_without_set() {
    let db = DatabaseStruct::default();
    db.double_length(());
}

#[test]
fn using_set_unchecked_on_input() {
    let db = DatabaseStruct::default();
    db.query(Input).set_unchecked((), format!("Hello, world"));
    assert_eq!(db.double_length(()), 24);
}

#[test]
fn using_set_unchecked_on_input_after() {
    let db = DatabaseStruct::default();
    db.query(Input).set((), format!("Hello, world"));
    assert_eq!(db.double_length(()), 24);

    // If we use `set_unchecked`, we don't notice that `double_length`
    // is out of date. Oh well, don't do that.
    db.query(Input).set_unchecked((), format!("Hello, world!"));
    assert_eq!(db.double_length(()), 24);
}

#[test]
fn using_set_unchecked() {
    let db = DatabaseStruct::default();

    // Use `set_unchecked` to intentionally set the wrong value,
    // demonstrating that the code never runs.
    db.query(Length).set_unchecked((), 24);

    assert_eq!(db.double_length(()), 48);
}
