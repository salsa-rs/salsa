#[salsa::jar(db = Db)]
struct Jar(Tracked<'_>);

#[salsa::tracked(jar = Jar)]
struct Tracked<'db> {
    field: u32,
}

impl<'db> Tracked<'db> {
    #[salsa::tracked]
    fn use_tracked(&self) {}
}

trait Db: salsa::DbWithJar<Jar> {}

fn main() {}
