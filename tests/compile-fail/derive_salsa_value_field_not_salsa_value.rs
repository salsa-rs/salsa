struct NotSalsaValue;

#[derive(salsa::SalsaValue)]
struct Value {
    field: NotSalsaValue,
}

fn main() {}
