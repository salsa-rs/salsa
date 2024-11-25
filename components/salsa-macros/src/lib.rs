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

/// Similar to `syn::parse_macro_input`, however, when a parse error is encountered, it will return
/// the input token stream in addition to the error. This will make it so that rust-analyzer can work
/// with incomplete code.
macro_rules! parse_macro_input {
    ($tokenstream:ident as $ty:ty) => {
        match syn::parse::<$ty>($tokenstream.clone()) {
            Ok(data) => data,
            Err(err) => {
                return $crate::token_stream_with_error($tokenstream, err);
            }
        }
    };
}

mod accumulator;
mod db;
mod db_lifetime;
mod debug;
mod fn_util;
mod hygiene;
mod input;
mod interned;
mod interned_sans_lifetime;
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
pub fn interned_sans_lifetime(args: TokenStream, input: TokenStream) -> TokenStream {
    interned_sans_lifetime::interned_sans_lifetime(args, input)
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
    let item = parse_macro_input!(input as syn::DeriveInput);
    match update::update_derive(item) {
        Ok(tokens) => tokens.into(),
        Err(error) => token_stream_with_error(input, error),
    }
}

pub(crate) fn token_stream_with_error(mut tokens: TokenStream, error: syn::Error) -> TokenStream {
    tokens.extend(TokenStream::from(error.into_compile_error()));
    tokens
}
