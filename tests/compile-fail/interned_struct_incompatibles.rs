#[salsa::jar(db = Db)]
struct Jar(InternedWithRetRef, InternedWithSpecify, InternedWithNoEq, InternedWithDb, InternedWithRecover, InternedWithLru);

trait Db: salsa::DbWithJar<Jar> {}


#[salsa::interned(jar = Jar, return_ref)]
struct InternedWithRetRef {
    field: u32,    
}

#[salsa::interned(jar = Jar, specify)]
struct InternedWithSpecify {
    field: u32,    
}

#[salsa::interned(jar = Jar, no_eq)]
struct InternedWithNoEq {
    field: u32,    
}

#[salsa::interned(jar = Jar, db = Db)]
struct InternedWithDb {
    field: u32,    
}

#[salsa::interned(jar = Jar, recover_fn = recover)]
struct InternedWithRecover {
    field: u32,    
}

#[salsa::interned(jar = Jar, lru =12)]
struct InternedWithLru {
    field: u32,    
}
fn main() {}