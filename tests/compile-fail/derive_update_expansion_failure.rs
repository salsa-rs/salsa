#[derive(salsa::Update)]
union U {
    field: i32,
}

#[derive(salsa::Update)]
struct S {
    #[update(with(missing_unsafe))]
    bad: i32,
}

fn missing_unsafe(_: *mut i32, _: i32) -> bool {
    true
}

fn main() {}
