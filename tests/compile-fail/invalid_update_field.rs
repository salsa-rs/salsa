#[derive(salsa::Update)]
struct S2<'a> {
    bad2: &'a str,
}

fn main() {}
