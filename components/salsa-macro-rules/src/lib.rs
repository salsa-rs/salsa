//! This crate defines various `macro_rules` macros
//! used as part of Salsa's internal plumbing.
//! These macros are re-exported under `salsa::plumbing``.
//! The procedural macros emit calls to these
//! `macro_rules` macros after doing error checking.
//!
//! Using `macro_rules` macro definitions is generally
//! more ergonomic and also permits true hygiene for local variables
//! (sadly not items).
//!
//! Currently the only way to have a macro that is re-exported
//! from a submodule is to use multiple crates, hence the existence
//! of this crate.

mod macro_if;
mod maybe_backdate;
mod maybe_clone;
mod maybe_default;
mod setup_accumulator_impl;
mod setup_input_struct;
mod setup_interned_struct;
mod setup_method_body;
mod setup_tracked_fn;
mod setup_tracked_struct;
mod unexpected_cycle_recovery;
