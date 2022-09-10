#[salsa::jar(db = Db)]
struct Jar(Tracked);


#[salsa::tracked(jar = Jar)]
struct Tracked {
    field: u32,    
}


impl Tracked {
    #[salsa::tracked]
    fn use_tracked(&self) {

    }
}

trait Db: salsa::DbWithJar<Jar> {}


fn main() {}