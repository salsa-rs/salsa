# How Salsa works

## Video available

To get the most complete introduction to Salsa's inner workings, check
out [the "How Salsa Works" video](https://youtu.be/_muY4HjSqVw). If
you'd like a deeper dive, [the "Salsa in more depth"
video](https://www.youtube.com/watch?v=i_IhACacPRY) digs into the
details of the incremental algorithm.

> If you're in China, watch videos on ["How Salsa Works"](https://www.bilibili.com/video/BV1Df4y1A7t3/), ["Salsa In More Depth"](https://www.bilibili.com/video/BV1AM4y1G7E4/).

## Key idea

The key idea of `salsa` is that you define your program as a set of
**queries**. Every query is used like a function `K -> V` that maps from
some key of type `K` to a value of type `V`. Queries come in two basic
varieties:

- **Inputs**: the base inputs to your system. You can change these
  whenever you like.
- **Functions**: pure functions (no side effects) that transform your
  inputs into other values. The results of queries are memoized to
  avoid recomputing them a lot. When you make changes to the inputs,
  we'll figure out (fairly intelligently) when we can re-use these
  memoized values and when we have to recompute them.

## How to use Salsa in three easy steps

Using Salsa is as easy as 1, 2, 3...

1. Define the **Salsa structs** you will need with `#[salsa::input]`,
   `#[salsa::tracked]`, or `#[salsa::interned]`.
2. Define your memoized **query functions** with `#[salsa::tracked]`.
3. Define the **database**, which contains a `salsa::Storage<Self>` field
   and may also contain anything else that your code needs.

To see an example of this in action, check out [the `calc` example][calc].

[calc]: https://github.com/salsa-rs/salsa/tree/master/examples/calc

## Digging into the plumbing

Check out the [plumbing](plumbing.md) chapter to see a deeper explanation of the
code that Salsa generates and how it connects to the Salsa library.
