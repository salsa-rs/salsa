//! Test that update queries work
use std::fmt::Write;
use std::rc::Rc;

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup {
    #[salsa::input]
    fn input(&self, x: u32) -> u32;
    #[salsa::update]
    fn formatted_value(&self, x: u32) -> Rc<String>;
    #[salsa::update(reverse_formatted_value_update)]
    fn reverse_formatted_value(&self, x: u32) -> Rc<String>;
}

fn formatted_value(db: &dyn QueryGroup, x: u32) -> Rc<String> {
    db.input(x).to_string().into()
}

fn update_formatted_value(db: &dyn QueryGroup, value: &mut Rc<String>, x: u32) -> salsa::ValueChanged {
    let value = Rc::make_mut(value);
    value.clear();

    let _ = write!(value, "{}", db.input(x));
    salsa::ValueChanged::True
}

fn reverse_formatted_value(db: &dyn QueryGroup, x: u32) -> Rc<String> {
    Rc::new(db.input(x).to_string().chars().rev().collect())
}

fn reverse_formatted_value_update(db: &dyn QueryGroup, value: &mut Rc<String>, x: u32) -> salsa::ValueChanged {
    let value = Rc::make_mut(value);
    value.clear();
    value.extend(db.formatted_value(x).chars().rev());
    salsa::ValueChanged::True
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

#[test]
fn update_queries_work() {
    let mut db = Database::default();

    db.set_input(1, 10);

    let value_ptr = {
        let value = db.formatted_value(1);
        assert_eq!(value.as_ref(), "10");
        assert_eq!(db.reverse_formatted_value(1).as_ref(), "01");
        value.as_bytes().as_ptr()
    };

    {
        let value2 = db.formatted_value(1);
        assert_eq!(value2.as_ref(), "10");
        assert_eq!(value_ptr, value2.as_bytes().as_ptr());
    }

    db.set_input(1, 92);

    {
        let value3 = db.formatted_value(1);
        assert_eq!(value3.as_ref(), "92");
        assert_eq!(value_ptr, value3.as_bytes().as_ptr());
    }

    {
        let value4 = db.formatted_value(1);
        assert_eq!(value4.as_ref(), "92");
        assert_eq!(value_ptr, value4.as_bytes().as_ptr());
    }
}
