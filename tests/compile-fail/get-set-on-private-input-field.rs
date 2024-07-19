use salsa::prelude::*;

mod a {
    #[salsa::input]
    pub struct MyInput {
        field: u32,
    }
}

fn main() {
    let mut db = salsa::default_database();
    let input = a::MyInput::new(&mut db, 22);

    input.field(&db);
    input.set_field(&mut db).to(23);
}
