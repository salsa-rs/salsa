# salsa

[![Build Status](https://travis-ci.org/salsa-rs/salsa.svg?branch=master)](https://travis-ci.org/salsa-rs/salsa)
[![Released API docs](https://docs.rs/salsa/badge.svg)](https://docs.rs/salsa)
[![Crates.io](https://img.shields.io/crates/v/salsa.svg)](https://crates.io/crates/salsa)

*A generic framework for on-demand, incrementalized computation.*

## Obligatory warning

Very much a WORK IN PROGRESS at this point. Ready for experimental use
but expect frequent breaking changes.

## Credits

This system is heavily inspired by adapton, glimmer, and rustc's query
system. So credit goes to Eduard-Mihai Burtescu, Matthew Hammer,
Yehuda Katz, and Michael Woerister.

## Key idea

The key idea of `salsa` is that you define your program as a set of
**queries**. Every query is used like function `K -> V` that maps from
some key of type `K` to a value of type `V`. Queries come in two basic
varieties:

- **Inputs**: the base inputs to your system. You can change these
  whenever you like.
- **Functions**: pure functions (no side effects) that transform your
  inputs into other values. The results of queries is memoized to
  avoid recomputing them a lot. When you make changes to the inputs,
  we'll figure out (fairly intelligently) when we can re-use these
  memoized values and when we have to recompute them.

## How to use Salsa in three easy steps

Using salsa is as easy as 1, 2, 3...

1. Define one or more **query groups** that contain the inputs
   and queries you will need. We'll start with one such group, but
   later on you can use more than one to break up your system into
   components (or spread your code across crates).
2. Define the **query functions** where appropriate.
3. Define the **database**, which contains the storage for all
   the inputs/queries you will be using. The query struct will contain
   the storage for all of the inputs/queries and may also contain
   anything else that your code needs (e.g., configuration data).

To see an example of this in action, check out [the `hello_world`
example](examples/hello_world/main.rs), which has a number of comments
explaining how things work. The [`hello_world`
README](examples/hello_world/README.md) has a more detailed writeup.

Salsa requires at least Rust 1.30 (beta at the time of writing).

## Getting in touch

The bulk of the discussion happens in the [issues](https://github.com/salsa-rs/salsa/issues) 
and [pull requests](https://github.com/salsa-rs/salsa/pulls), 
but we have a [zulip chat](https://salsa.zulipchat.com/) as well.

