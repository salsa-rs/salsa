trait Db: salsa::DbWithJar<Jar> {}

#[salsa::jar(db = Db)]
struct Jar(Keywords);

#[salsa::interned(jar = Jar)]
struct Keywords {}
