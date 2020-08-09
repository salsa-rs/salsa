
## Metadata

* Author: marwes
* Date: 2020-08-04
* Introduced in: [salsa-rs/salsa#1](https://github.com/salsa-rs/salsa/pull/1) (please update once you open your PR)

## Summary

Allow salsa databases to define derived queries that execute asynchronous code.

## Motivation

Asynchronous code is typically not needed for compilers as they typically do not need to wait on IO. However there is still utility in writing asynchronous queries
as asynchronous tasks model queries waiting for eachother really well. There may also be non-compiler uses for salsa where the async nature may be useful (think caching a resource fetch over the network).

### Our goal

Allow `async fn` to be written for any query that needs to be `async`.

## User's guide

Any salsa query that involves running user code (not input or interned queries) can now be written as an `async fn`. These queries accept the database as an `&mut salsa::OwnedDb<'_, dyn MyDatabase>` (or `&mut dyn MyDatabase` as a convenience for the top query) object instead of `&dyn MyDatabase` as a way to preserve `Send` for the returned futures but are otherwise identical.

```rust
#[salsa::query_group(AsyncStorage)]
trait Async: Send {
    async fn query(&self, x: u32) -> u32;
}

async fn query(db: &mut salsa::OwnedDb<'_, dyn Async>, x: u32) -> u32 {
    async_function(db, x, "abc").await
}
```

## Reference guide

To avoid duplication all operations on derived queries are now written as `async fn`. Since these `async` functions end up capturing the database parameter and we want the returned futures to implement `Send` we need to change these functions so they accept `&mut` instead of `&`. But we also do not want a running query to use any `&mut` operations. To resolve this the internal `QueryDb` is extended with a `Db` type that is used to parameterize queries over `&dyn MyDatabase` and `salsa::OwnedDb<'_, dyn MyDatabase>`. The latter is used for `async` queries and works in a similar way to `Snapshot`. User code can only retrieve a `&` reference, while the salsa internals can still retrieve the `&mut` reference within (as such there is also a `From` conversion from `Snapshot` to `OwnedDb`).

```rust
pub trait QueryDb<'d>: QueryBase {
    /// Dyn version of the associated trait for this query group.
    type DynDb: ?Sized + Database + HasQueryGroup<Self::Group> + 'd;

    /// Sized version of `DynDb`, &'d Self::DynDb for synchronous queries
    type Db: std::ops::Deref<Target = Self::DynDb> + AsAsyncDatabase<Self::DynDb>;
}
```

The only internal operation in a query that blocks for a "long" time is waiting for a query that is currently executing. For async queries we want to yield to the executor here instead of blocking whereas synchronous queries want to just block the thread. While we could just use an async-aware primitive for this and invoke synchronous queries with an executor capable of handling this that would force a small but potentially noticeable overhead for synchronous queries. 

To resolve this we can instead parameterize over this `BlockingFuture` with a type which just blocks for synchronous queries and uses a real async-aware type for async queries. This then allows for a truly minimal "executor" which doesn't need to handle `Poll::Pending` at all

```rust
/// Calls a future synchronously without an actual way to resume to future.
pub(crate) fn sync_future<F>(mut f: F) -> F::Output
where
    F: Future,
{
    use std::task::{RawWaker, RawWakerVTable, Waker};

    unsafe {
        type WakerState = ();

        static VTABLE: RawWakerVTable =
            RawWakerVTable::new(|p| RawWaker::new(p, &VTABLE), |_| (), |_| (), |_| ());

        let waker_state = WakerState::default();
        let waker = Waker::from_raw(RawWaker::new(
            &waker_state as *const WakerState as *const (),
            &VTABLE,
        ));
        let mut context = Context::from_waker(&waker);

        match Pin::new_unchecked(&mut f).poll(&mut context) {
            Poll::Ready(x) => x,
            Poll::Pending => unreachable!(),
        }
    }
}
```

The `QueryFunction` trait which represented the user code actually being invoke are changed to return a `Future`. For `async` queries this must be a boxed future but for synchronous queries this can just be the non-allocating `Ready` future that were ported from the `futures` crate to avoid a dependency on it.

```rust
pub trait QueryFunctionBase: QueryBase {
    type BlockingFuture: BlockingFutureTrait<WaitResult<Self::Value, DatabaseKeyIndex>>;
}

pub trait QueryFunction<'f, 'd>: QueryFunctionBase + QueryDb<'d> {
    type Future: Future<Output = Self::Value> + 'f;

    fn execute(db: &'f mut <Self as QueryDb<'d>>::Db, key: Self::Key) -> Self::Future;
    ...
}
```

The functions `QueryStorageOps::try_fetch` and `QueryStorageOps::maybe_changed_since` were extracted to a `QueryStorageOpsSync` trait and a matching `QueryStorageOpsAsync` trait were added. Through this split the synchronous code does not need to allocate the boxed future that the async variant must return and it also ensures that other queries such as `input` never implements the `async` functions.

## Alternatives and future work

The slightly awkward `OwnedDb` could perhaps be avoided if salsa's `RefCell` usage were removed. However my attempts so far hasn't yielded a workable solution.