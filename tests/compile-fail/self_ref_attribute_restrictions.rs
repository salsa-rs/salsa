#[salsa::input]
struct InputWithSelfRef {
    #[self_ref]
    field: u32,
}

#[salsa::tracked]
struct TrackedWithSelfRef {
    #[self_ref]
    field: u32,
}

#[salsa::interned]
struct SelfRefWithArguments {
    #[self_ref(other)]
    field: u32,
}

fn main() {}
