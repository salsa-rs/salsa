#![cfg(feature = "inventory")]

//! Test that a `tracked` fn on a `salsa::interned`
//! compiles and executes successfully.

#[salsa::interned(no_lifetime, debug)]
struct Name {
    name: String,
}

#[salsa::interned(debug)]
struct NameAndAge<'db> {
    name_and_age: String,
}

#[salsa::interned(no_lifetime, debug)]
struct Age {
    age: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
enum Enum<'db> {
    Name(Name),
    NameAndAge(NameAndAge<'db>),
    Age(Age),
}

#[salsa::input(debug)]
struct Input {
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
enum EnumOfEnum<'db> {
    Enum(Enum<'db>),
    Input(Input),
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, enum_: Enum<'db>) -> String {
    match enum_ {
        Enum::Name(name) => name.name(db),
        Enum::NameAndAge(name_and_age) => name_and_age.name_and_age(db),
        Enum::Age(age) => age.age(db).to_string(),
    }
}

#[salsa::tracked]
fn tracked_fn2<'db>(db: &'db dyn salsa::Database, enum_: EnumOfEnum<'db>) -> String {
    match enum_ {
        EnumOfEnum::Enum(enum_) => tracked_fn(db, enum_),
        EnumOfEnum::Input(input) => input.value(db),
    }
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();
    let name = Name::new(&db, "Salsa".to_string());
    let name_and_age = NameAndAge::new(&db, "Salsa 3".to_string());
    let age = Age::new(&db, 123);

    assert_eq!(tracked_fn(&db, Enum::Name(name)), "Salsa");
    assert_eq!(tracked_fn(&db, Enum::NameAndAge(name_and_age)), "Salsa 3");
    assert_eq!(tracked_fn(&db, Enum::Age(age)), "123");
    assert_eq!(tracked_fn(&db, Enum::Name(name)), "Salsa");
    assert_eq!(tracked_fn(&db, Enum::NameAndAge(name_and_age)), "Salsa 3");
    assert_eq!(tracked_fn(&db, Enum::Age(age)), "123");

    assert_eq!(
        tracked_fn2(&db, EnumOfEnum::Enum(Enum::Name(name))),
        "Salsa"
    );
    assert_eq!(
        tracked_fn2(&db, EnumOfEnum::Enum(Enum::NameAndAge(name_and_age))),
        "Salsa 3"
    );
    assert_eq!(tracked_fn2(&db, EnumOfEnum::Enum(Enum::Age(age))), "123");
    assert_eq!(
        tracked_fn2(&db, EnumOfEnum::Enum(Enum::Name(name))),
        "Salsa"
    );
    assert_eq!(
        tracked_fn2(&db, EnumOfEnum::Enum(Enum::NameAndAge(name_and_age))),
        "Salsa 3"
    );
    assert_eq!(tracked_fn2(&db, EnumOfEnum::Enum(Enum::Age(age))), "123");
    assert_eq!(
        tracked_fn2(
            &db,
            EnumOfEnum::Input(Input::new(&db, "Hello world!".to_string()))
        ),
        "Hello world!"
    );
}
