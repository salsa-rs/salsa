//@compile-fail
#![deny(warnings)]

#[salsa::interned(data = ConflictingData)]
enum ConflictingData<'db> {
    Variant(&'db ()),
}

fn main() {}
