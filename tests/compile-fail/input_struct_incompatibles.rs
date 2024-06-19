#[salsa::jar(db = Db)]
struct Jar(InputWithRetRef, InputWithSpecify, InputNoWithEq, InputWithDb, InputWithRecover, InputWithLru);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input(jar = Jar, return_ref)]
struct InputWithRetRef (u32);

#[salsa::input(jar = Jar, specify)]
struct InputWithSpecify (u32);

#[salsa::input(jar = Jar, no_eq)]
struct InputNoWithEq (u32);

#[salsa::input(jar = Jar, db = Db)]
struct InputWithDb (u32);

#[salsa::input(jar = Jar, recover_fn = recover)]
struct InputWithRecover (u32);

#[salsa::input(jar = Jar, lru =12)]
struct InputWithLru (u32);

fn main() {}