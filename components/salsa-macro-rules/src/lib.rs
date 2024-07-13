//! This crate defines various `macro_rules` macros
//! used as part of Salsa's internal plumbing.
//!
//! The procedural macros typically emit calls to these
//! `macro_rules` macros.
//!
//! Modifying `macro_rules` macro definitions is generally
//! more ergonomic and also permits true hygiene.

mod setup_interned_fn;
mod setup_tracked_struct;
mod unexpected_cycle_recovery;

#[macro_export]
macro_rules! setup_fn {
    () => {};
}
