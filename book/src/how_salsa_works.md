# How Salsa works

## Video available

To get the most complete introduction to Salsa's inner works, check
out [the "How Salsa Works" video](https://youtu.be/_muY4HjSqVw).  If
you'd like a deeper dive, [the "Salsa in more depth"
video](https://www.youtube.com/watch?v=i_IhACacPRY) digs into the
details of the incremental algorithm.

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
example][hello_world], which has a number of comments explaining how
things work.

[hello_world]: https://github.com/salsa-rs/salsa/blob/master/examples/hello_world/main.rs

## Digging into the plumbing

Check out the [plumbing](plumbing.md) chapter to see a deeper explanation of the
code that salsa generates and how it connects to the salsa library.