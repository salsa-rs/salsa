# Tutorial: calc

This tutorial walks through an end-to-end example of using Salsa.
It does not assume you know anything about salsa,
but reading the [overview](./overview.md) first is probably a good idea to get familiar with the basic concepts.

Our goal is define a compiler/interpreter for a simple language called `calc`.
The `calc` compiler takes programs like the following and then parses and executes them:

```
fn area_rectangle(w, h) = w * h
fn area_circle(r) = 3.14 * r * r
print area_rectangle(3, 4)
print area_circle(1)
print 11 * 2
```

When executed, this program prints `12`, `3.14`, and `22`.

If the program contains errors (e.g., a reference to an undefined function), it prints those out too.
And, of course, it will be reactive, so small changes to the input don't require recompiling (or rexecuting, necessarily) the entire thing.
