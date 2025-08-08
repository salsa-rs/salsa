#![cfg(feature = "inventory")]

#[rustversion::all(stable, since(1.89))]
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
