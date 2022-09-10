#[salsa::jar(db = Db, return_ref)]
struct JarWithRetRef(MyInput);

#[salsa::jar(db = Db, specify)]
struct JarWithDb(MyInput);


#[salsa::jar(db = Db, no_eq)]
struct JarWithNoEq(MyInput);

#[salsa::jar(db = Db, jar = Jar)]
struct JarWithJar(MyInput);

#[salsa::jar(db = Db, data = Data)]
struct JarWithData(MyInput);

#[salsa::jar(db = Db, recovery_fn = recover)]
struct JarWithRecover(MyInput);

#[salsa::jar(db = Db, lru = 32)]
struct JarWithLru(MyInput);

#[salsa::jar(db = Db, constructor = JarConstructor)]
struct JarWithConstructor(MyInput);

#[salsa::input(jar = Jar1)]
struct MyInput {
    field: u32,
}

fn main() {

}