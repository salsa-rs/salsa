use salsa::Database as Db;

#[salsa::input]
struct MyInput {
    #[returns(copy)]
    field: u32,
}

#[derive(Clone)]
struct NotUpdate;

#[salsa::tracked(volatile = 3, returns(clone))]
fn volatile_requires_update(_db: &dyn Db, _input: MyInput) -> NotUpdate {
    unreachable!()
}

#[salsa::tracked(volatile = 3, lru = 3)]
fn volatile_with_lru(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(volatile = 3, sieve = 3)]
fn volatile_with_sieve(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(volatile = 3, specify)]
fn volatile_with_specify(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(volatile = 3, unsafe(non_update_types))]
fn volatile_with_non_update(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(volatile = 3, returns(ref))]
fn volatile_with_ref(db: &dyn Db, input: MyInput) -> String {
    input.field(db).to_string()
}

fn main() {}
