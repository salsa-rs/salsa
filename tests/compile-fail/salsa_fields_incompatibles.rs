#[salsa::jar(db = Db)]
struct Jar(InputWithBannedName1, InputWithBannedName2);

// Banned field name: `from`
#[salsa::input]
struct InputWithBannedName1 {
    from: u32,
}

// Banned field name: `new`
#[salsa::input]
struct InputWithBannedName2 {
    new: u32,
}

trait Db: salsa::DbWithJar<Jar> {}


fn main() {}