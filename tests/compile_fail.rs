#![cfg(all(feature = "inventory", feature = "persistence"))]

// Diagnostic snapshots match the current stable compiler, not the MSRV.
#[rustversion::all(stable, since(1.99))]
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
