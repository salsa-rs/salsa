#[salsa::interned(return_ref)]
struct InternedWithRetRef {
    field: u32,
}

#[salsa::interned(specify)]
struct InternedWithSpecify {
    field: u32,
}

#[salsa::interned(no_eq)]
struct InternedWithNoEq {
    field: u32,
}

#[salsa::interned(db = Db)]
struct InternedWithDb {
    field: u32,
}

#[salsa::interned(recover_fn = recover)]
struct InternedWithRecover {
    field: u32,
}

#[salsa::interned(lru = 12)]
struct InternedWithLru {
    field: u32,
}

#[salsa::interned]
struct InternedWithIdField {
    #[id]
    field: u32,
}

fn main() {}
