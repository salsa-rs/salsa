#[derive(salsa::SalsaValue)]
struct Generic<T>(T);

#[derive(salsa::SalsaValue)]
struct ConstGeneric<const N: usize>([u8; N]);

fn main() {}
