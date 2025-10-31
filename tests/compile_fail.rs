#![cfg(all(feature = "inventory", feature = "persistence"))]

#[rustversion::all(stable)]
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
