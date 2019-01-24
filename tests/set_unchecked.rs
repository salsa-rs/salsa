use salsa::Database;

#[salsa::query_group]
trait HelloWorldDatabase: salsa::Database {
    #[salsa::input]
    fn input(&self) -> String;

    fn length(&self) -> usize;

    fn double_length(&self) -> usize;
}

fn length(db: &impl HelloWorldDatabase) -> usize {
    let l = db.input().len();
    assert!(l > 0); // not meant to be invoked with no input
    l
}

fn double_length(db: &impl HelloWorldDatabase) -> usize {
    db.length() * 2
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
    DatabaseStruct {
        impl HelloWorldDatabase;
    }
}

#[test]
fn normal() {
    let mut db = DatabaseStruct::default();
    db.query_mut(InputQuery).set((), format!("Hello, world"));
    assert_eq!(db.double_length(), 24);
    db.query_mut(InputQuery).set((), format!("Hello, world!"));
    assert_eq!(db.double_length(), 26);
}

#[test]
#[should_panic]
fn use_without_set() {
    let db = DatabaseStruct::default();
    db.double_length();
}

#[test]
fn using_set_unchecked_on_input() {
    let mut db = DatabaseStruct::default();
    db.query_mut(InputQuery)
        .set_unchecked((), format!("Hello, world"));
    assert_eq!(db.double_length(), 24);
}

#[test]
fn using_set_unchecked_on_input_after() {
    let mut db = DatabaseStruct::default();
    db.query_mut(InputQuery).set((), format!("Hello, world"));
    assert_eq!(db.double_length(), 24);

    // If we use `set_unchecked`, we don't notice that `double_length`
    // is out of date. Oh well, don't do that.
    db.query_mut(InputQuery)
        .set_unchecked((), format!("Hello, world!"));
    assert_eq!(db.double_length(), 24);
}

#[test]
fn using_set_unchecked() {
    let mut db = DatabaseStruct::default();

    // Use `set_unchecked` to intentionally set the wrong value,
    // demonstrating that the code never runs.
    db.query_mut(LengthQuery).set_unchecked((), 24);

    assert_eq!(db.double_length(), 48);
}
