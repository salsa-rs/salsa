trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
struct Jar(Keywords<'_>);

#[salsa::interned]
struct Keywords<'db> {}
