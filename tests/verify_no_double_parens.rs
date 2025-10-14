//! Compile-time verification test for clippy::double_parens fix.
//!
//! This test verifies that the expanded macros don't contain the problematic
//! std::mem::drop(()) pattern that triggers clippy warnings.

use std::process::Command;

#[test]
fn verify_expanded_code_has_no_double_parens() {
    let output = Command::new("cargo")
        .args(["expand", "--test", "clippy_double_parens_regression"])
        .output()
        .expect("Failed to execute cargo expand");

    let expanded = String::from_utf8_lossy(&output.stdout);

    // Filter out lines that are comments or documentation to avoid false positives
    let code_lines: Vec<&str> = expanded
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("//") && !trimmed.starts_with("///")
        })
        .collect();

    let code_only = code_lines.join("\n");

    // Check for the problematic pattern in the code (ignore comments)
    if code_only.contains("std::mem::drop(())") {
        panic!(
            "Found std::mem::drop(()) in expanded code.\n\
             This pattern triggers clippy::double_parens warnings.\n\
             The macro should use $(std::mem::drop($other_inputs);)* \
             instead of std::mem::drop(($($other_inputs,)*))"
        );
    }
}
