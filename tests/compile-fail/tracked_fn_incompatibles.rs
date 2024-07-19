use salsa::Database as Db;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked(data = Data)]
fn tracked_fn_with_data(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::tracked(db = Db)]
fn tracked_fn_with_db(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::tracked(constructor = TrackedFn3)]
fn tracked_fn_with_constructor(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::tracked]
fn tracked_fn_with_one_input(db: &dyn Db) -> u32 {}

#[salsa::tracked]
fn tracked_fn_with_receiver_not_applied_to_impl_block(&self, db: &dyn Db) -> u32 {}

#[salsa::tracked(specify)]
fn tracked_fn_with_too_many_arguments_for_specify(
    db: &dyn Db,
    input: MyInput,
    input: MyInput,
) -> u32 {
}

fn main() {}
