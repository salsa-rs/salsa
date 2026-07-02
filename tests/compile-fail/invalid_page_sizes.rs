#[salsa::interned(page_size = 127)]
struct NonPowerOfTwoPageSize<'db> {
    value: usize,
}

#[salsa::input(page_size = 64 * 2)]
struct ExpressionPageSize {
    value: usize,
}

fn main() {}
