#[salsa::jar(db = Db)]
struct Jar(MyInput, tracked_fn_with_data, tracked_fn_with_db, tracked_fn_with_constructor, tracked_fn_with_one_input, tracked_fn_with_receiver_not_applied_to_impl_block);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}


#[salsa::tracked(jar = Jar, data = Data)]
fn tracked_fn_with_data(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::tracked(jar = Jar, db = Db)]
fn tracked_fn_with_db(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::tracked(jar = Jar, constructor = TrackedFn3)]
fn tracked_fn_with_constructor(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}


#[salsa::tracked(jar = Jar)]
fn tracked_fn_with_one_input(db: &dyn Db) -> u32 {
}


#[salsa::tracked(jar = Jar)]
fn tracked_fn_with_receiver_not_applied_to_impl_block(&self, db: &dyn Db) -> u32 {
}

#[salsa::tracked(jar = Jar, specify)]
fn tracked_fn_with_receiver_not_applied_to_impl_block(db: &dyn Db, input: MyInput, input: MyInput) -> u32 {
}







fn main() {}