# Opinionated cancelation

## Metadata

* Author: nikomatsakis
* Date: 2021-05-15
* Introduced in: [salsa-rs/salsa#265](https://github.com/salsa-rs/salsa/pull/265)

## Summary

* Define stack unwinding as the one true way to handle cancelation in salsa queries
* Modify salsa queries to automatically initiate unwinding when they are canceled
* Use a distinguished value for this panic so that people can test if the panic was a result of cancelation

## Motivation

Salsa's database model is fundamentally like a read-write lock. There is always a single *master copy* of the database which supports writes, and any number of concurrent *snapshots* that support reads. Whenever a write to the database occurs, any queries executing in those snapshots are considered *canceled*, because their results are based on stale data. The write blocks until they complete before it actually takes effect. It is therefore advantageous for those reads to complete as quickly as possible.

cancelation in salsa is currently quite minimal. Effectively, a flag becomes true, and queries can manually check for this flag. This is easy to forget to do. Moreover, we support two modes of cancelation: you can either use `Result` values or use unwinding. In practice, though, there isn't much point to using `Result`: you can't really "recover" from cancelation.

The largest user of salsa, rust-analyzer, uses a fairly opinionated and aggressive form of cancelation:

* Every query is instrumented, using salsa's various hooks, to check for cancelation before it begins.
* If a query is canceled, then it immediately panics, using a special sentinel value.
* Any worker threads holding a snapshot of the DB recognize this value and go back to waiting for work.

We propose to make this model of cancelation the *only* model of cancelation.

## User's guide

When you do a write to the salsa database, that write will block until any queries running in background threads have completed. You really want those queries to complete quickly, though, because they are now operating on stale data and their results are therefore not meaningful. To expedite the process, salsa will *cancel* those queries. That means that the queries will panic as soon as they try to execute another salsa query. Those panics occur using a sentinel value that you can check for if you wish. If you have a query that contains a long loop which does not execute any intermediate queries, salsa won't be able to cancel it automatically. You may wish to check for cancelation yourself by invoking the `unwind_if_canceled` method.

## Reference guide

The changes required to implement this RFC are as follows:

* Remove on `is_current_revision_canceled`.
* Introduce a sentinel cancellation token that can be used with [`resume_unwind`](https://doc.rust-lang.org/std/panic/fn.resume_unwind.html)
* Introduce a `unwind_if_canceled` method into the `Database` which checks whether cancelation has occured and panics if so.
    * This method also triggers a `salsa_event` callback.
    * This should probably be inline for the `if` with an outlined function to do the actual panic.
* Modify the code for the various queries to invoke `unwind_if_canceled` when they are invoked or validated.

## Frequently asked questions

### Isn't it hard to write panic-safe code?

It is. However, the salsa runtime is panic-safe, and all salsa queries must already avoid side-effects for other reasons, so in our case, being panic-safe happens by default.

### Isn't recovering from panics a bad idea?

No. It's a bad idea to do "fine-grained" recovery from panics, but catching a panic at a high-level of your application and soldiering on is actually exactly how panics were meant to be used. This is especially true in salsa, since all code is already panic-safe.

### Does this affect users of salsa who do not use threads?

No. Cancelation in salsa only occurs when there are parallel readers and writers.

### What about people using panic-as-abort?

This does mean that salsa is not compatible with panic-as-abort. Strictly speaking, you could still use salsa in single-threaded mode, so that cancelation is not possible.


