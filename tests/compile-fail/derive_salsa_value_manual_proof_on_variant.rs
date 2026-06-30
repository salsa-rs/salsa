#[derive(salsa::SalsaValue)]
enum Value {
    #[salsa_value(prove_safe_to_retain_manually)]
    Variant,
}

fn main() {}
