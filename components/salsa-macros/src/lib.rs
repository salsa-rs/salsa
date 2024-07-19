//! This crate provides salsa's macros and attributes.

#![recursion_limit = "256"]

extern crate proc_macro;
extern crate proc_macro2;
#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

macro_rules! parse_quote {
    ($($inp:tt)*) => {
        {
            let tt = quote!{$($inp)*};
            syn::parse2(tt.clone()).unwrap_or_else(|err| {
                panic!("failed to parse `{}` at {}:{}:{}: {}", tt, file!(), line!(), column!(), err)
            })
        }
    }
}

mod accumulator;
mod db;
mod db_lifetime;
mod debug;
mod fn_util;
mod hygiene;
mod input;
mod interned;
mod options;
mod salsa_struct;
mod tracked;
mod tracked_fn;
mod tracked_impl;
mod tracked_struct;
mod update;
mod xform;

#[proc_macro_attribute]
pub fn accumulator(args: TokenStream, input: TokenStream) -> TokenStream {
    accumulator::accumulator(args, input)
}

#[proc_macro_attribute]
pub fn db(args: TokenStream, input: TokenStream) -> TokenStream {
    db::db(args, input)
}

#[proc_macro_attribute]
pub fn interned(args: TokenStream, input: TokenStream) -> TokenStream {
    interned::interned(args, input)
}

#[proc_macro_attribute]
pub fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    input::input(args, input)
}

#[proc_macro_attribute]
pub fn tracked(args: TokenStream, input: TokenStream) -> TokenStream {
    tracked::tracked(args, input)
}

#[proc_macro_derive(Update)]
pub fn update(input: TokenStream) -> TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    match update::update_derive(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
