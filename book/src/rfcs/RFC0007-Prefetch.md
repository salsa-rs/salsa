# Description/title

## Metadata

* Author: nikomatsakis
* Date: 2020-07-10
* Introduced in: https://github.com/salsa-rs/salsa/pull/1 (please update once you open your PR)

## Summary

* Add a "prefetch" operation for queries

## Motivation

While Salsa supports having multiple active snapshots on the same database, it
doesn't really offer a convenient way for users to do things in parallel. This
was partly "by design" -- Salsa doesn't want to be starting or managing threads
on its own. It would be better if the "host" of the Salsa environment is
responsible for that.

But there is a need for queries to be able to say things like:

* The next step is to type check these N functions in parallel. These queries
  can execute in any order.
* Or, I am going to do X and then do Y, which are independent and could run in
  parallel.

This RFC proposes a building block, the **prefetch** opreation. The basic idea
is that users can "prefetch" a query. This has no "semantics" in particular, it
doesn't force anything to happen, and it yields a `()` result. However, it
serves as a **hint** that the query should begin executing in a parallel thread,
if one is available.

The intended pattern is that the user "prefetches" queries and then later
executes them for real. If all goes well, the query will already have a cached
result waitin for them.

To actually execute pre-fetches, the database will be extended with some methods
to help execute pre-fetched queries. The intention is that the host can start up
some 'helper threads' and put them to work, or connect the pre-fetches to a
thread-pool.

## User's guide

There are two parts to the design.

### Requesting a prefetch

We extend the query table with a `prefetch` method, so that users can do
`QueryType.in_db(db).prefetch(key)`. This has `()` result.

Queries can also be annotated with `#[prefetch]` in the query group, in which
case we will generate a `prefetch_query` method, so that users can simply write
`db.prefetch_query(key)`.

**Question:** Should we make the `#[prefetch]` annotation mandatory in order
to enabling pre-fetching

### Database interface

XXX

## Reference guide

### Interaction with database snapshots



### Prefetching

When 

## Alternatives and future work

Various downsides, rejected approaches, or other considerations.

