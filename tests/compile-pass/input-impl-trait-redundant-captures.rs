#![deny(impl_trait_redundant_captures)]

#[salsa::input]
struct Input {
    field: u32,
}

fn main() {}
