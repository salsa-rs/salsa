#[salsa::input]
struct MyInput {
    #[returns(copy)]
    field: u32,
}

#[salsa::tracked(eviction(policy = lru, capacity = 3), specify)]
fn eviction_can_not_be_used_with_specify(db: &dyn salsa::Database, input: MyInput) -> u32 {
    input.field(db)
}

fn main() {}
