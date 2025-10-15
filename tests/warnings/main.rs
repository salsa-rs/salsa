//! Test that macros don't generate code with warnings

#![deny(warnings)]

mod double_parens;
mod needless_borrow;
mod needless_lifetimes;
mod unused_variable_db;
