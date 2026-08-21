#[salsa::interned]
struct Other<'db> {
    value: u32,
}

#[salsa::interned]
struct Bad<'db> {
    key: u32,
    #[self_ref]
    other: Other<'db>,
}

fn main() {}
