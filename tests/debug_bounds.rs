#![cfg(feature = "inventory")]

//! Test that debug and non-debug structs compile correctly

#[derive(Ord, PartialOrd, Eq, PartialEq, Copy, Clone, Hash)]
struct NotDebug;
#[derive(Ord, PartialOrd, Eq, PartialEq, Copy, Clone, Hash, Debug)]
struct Debug;

#[salsa::input(debug)]
struct DebugInput {
    field: Debug,
}

#[salsa::input]
struct NotDebugInput {
    field: NotDebug,
}

#[salsa::interned(debug)]
struct DebugInterned {
    field: Debug,
}

#[salsa::interned]
struct NotDebugInterned {
    field: NotDebug,
}

#[salsa::interned(no_lifetime, debug)]
struct DebugInternedNoLifetime {
    field: Debug,
}

#[salsa::interned(no_lifetime)]
struct NotDebugInternedNoLifetime {
    field: NotDebug,
}

#[salsa::tracked(debug)]
struct DebugTracked<'db> {
    field: Debug,
}

#[salsa::tracked]
struct NotDebugTracked<'db> {
    field: NotDebug,
}

#[test]
fn ok() {}
