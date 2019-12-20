use std::sync::Arc;

mod compiler;
mod implementation;
mod interner;
mod values;

use self::compiler::{Compiler, CompilerMut};
use self::implementation::DatabaseImpl;
use self::interner::Interner;

static INPUT_STR: &'static str = r#"
lorem,ipsum
dolor,sit,amet,
consectetur,adipiscing,elit
"#;

#[test]
fn test() {
    let mut db = DatabaseImpl::default();

    db.set_input_string(Arc::new(INPUT_STR.to_owned()));

    let all_fields = db.all_fields();
    assert_eq!(
        format!("{:?}", all_fields),
        "[Field(0), Field(1), Field(2), Field(3), Field(4), Field(5), Field(6), Field(7)]"
    );
}

fn main() {
    let mut db = DatabaseImpl::default();

    db.set_input_string(Arc::new(INPUT_STR.to_owned()));

    for field in db.all_fields().iter() {
        let field_data = db.lookup_intern_field(*field);
        println!("{:?} => {:?}", field, field_data);
    }
}
