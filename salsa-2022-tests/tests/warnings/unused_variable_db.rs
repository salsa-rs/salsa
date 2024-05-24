trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
struct Jar(Keywords<'_>);

#[salsa::interned(jar = Jar)]
struct Keywords<'db> {}
