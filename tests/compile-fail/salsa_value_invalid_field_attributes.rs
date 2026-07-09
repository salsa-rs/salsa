#[derive(salsa::SalsaValue)]
struct Duplicate {
    #[salsa_value(unsafe(prove_safe_to_retain_manually))]
    #[salsa_value(unsafe(prove_safe_to_retain_manually))]
    field: String,
}

#[derive(salsa::SalsaValue)]
struct Malformed {
    #[salsa_value(prove_safe_to_retain_manually)]
    field: String,
}

#[salsa::input]
struct Disallowed {
    #[salsa_value(unsafe(prove_safe_to_retain_manually))]
    field: String,
}

fn main() {}
