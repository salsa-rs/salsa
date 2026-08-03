//! Test that macros don't generate code with warnings

#![deny(warnings)]

mod needless_borrow;
mod needless_lifetimes;
mod salsa_value_unused_lifetimes;
mod unused_variable_db;
