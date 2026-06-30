#[derive(salsa::SalsaValue)]
struct Duplicate {
    #[salsa_value(prove_safe_to_retain_manually)]
    #[salsa_value(prove_safe_to_retain_manually)]
    field: String,
}

#[derive(salsa::SalsaValue)]
struct Malformed {
    #[salsa_value(not_a_manual_proof)]
    field: String,
}

#[salsa::input]
struct Disallowed {
    #[salsa_value(prove_safe_to_retain_manually)]
    field: String,
}

fn main() {}
