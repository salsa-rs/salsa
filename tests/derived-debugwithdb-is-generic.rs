//! Tests that the impls of `DebugWithDb` in `derive`s and for Salsa structs
//! is fully generic, rather than using dyn trait.

use salsa::database::AsSalsaDatabase;
use salsa::DbWithJar;
use salsa::DebugWithDb;

use std::marker::PhantomData;

#[salsa::jar(db = Db)]
struct Jar(MyInputStruct, MyTrackedStruct<'_>, MyInternedStruct<'_>);

trait Db: salsa::DbWithJar<Jar> {}
impl<DB> Db for DB where DB: ?Sized + salsa::DbWithJar<Jar> {}

#[salsa::input]
struct MyInputStruct {}

#[salsa::tracked]
struct MyTrackedStruct<'db> {}

#[salsa::interned]
struct MyInternedStruct<'db> {}

#[derive(salsa::DebugWithDb)]
struct MyDerivedStruct<'db> {
    _phantom: PhantomData<&'db ()>,
}

fn ensure_generic<DB: ?Sized + AsSalsaDatabase + DbWithJar<Jar>, S: DebugWithDb<DB>>() {}

#[allow(unused)]
fn test_all<'db, DB: ?Sized + AsSalsaDatabase + DbWithJar<Jar>>() {
    ensure_generic::<DB, MyInputStruct>();
    ensure_generic::<DB, MyTrackedStruct<'db>>();
    ensure_generic::<DB, MyInternedStruct<'db>>();
    ensure_generic::<DB, MyDerivedStruct<'db>>();
}

#[test]
fn execute() {
    // We just need to make sure this test compiles.
    // No need to actually run it.
}
