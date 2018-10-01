# salsa

*A generic framework for on-demand, incrementalized computation.*

## Obligatory warning

Very much a WORK IN PROGRESS at this point. Ready for experimental use
but expect frequent breaking changes.

## Credits

This system is heavily inspired by adapton, glimmer, and rustc's query
system. So credit goes to Eduard-Mihai Burtescu, Matthew Hammer,
Yehuda Katz, and Michael Woerister.

## Goals

It tries to hit a few goals:

- No need for a base crate that declares the "complete set of queries"
- Each query can define its own storage and doesn't have to be memoized
- Each module only has to know about the queries that it depends on
  and that it provides (but no others)
- Compiles to fast code, with no allocation, dynamic dispatch, etc on
  the "memoized hit" fast path
- Can recover from cycles gracefully (though I didn't really show
  that)
- Should support arenas and other lifetime-based things without requiring
  lifetimes everywhere when you're not using them (untested)

## Example

There is a working `hello_world` example which is probably the best documentation.
More to come when I expand out a few more patterns.
