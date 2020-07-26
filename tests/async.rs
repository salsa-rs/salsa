#![cfg(feature = "async")]
use std::task::Poll;

#[salsa::database(async AsyncStorage)]
#[derive(Default)]
struct AsyncDatabase {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for AsyncDatabase {}

#[salsa::query_group(AsyncStorage)]
trait Async: Send {
    #[salsa::input]
    fn input(&self, x: u32) -> u32;

    async fn output(&self, x: u32) -> u32;

    #[salsa::cycle(recover)]
    async fn output_inner(&self, x: u32) -> u32;

    #[salsa::dependencies]
    async fn output_dependencies(&self, x: u32) -> u32;

    #[salsa::transparent]
    async fn output_transparent(&self, x: u32) -> u32;
}

fn recover(_: &dyn Async, _: &[String], _: &u32) -> u32 {
    0
}

async fn output(db: &mut OwnedAsync<'_>, x: u32) -> u32 {
    yield_().await;
    db.output_inner(x).await
}

async fn output_inner(db: &mut OwnedAsync<'_>, x: u32) -> u32 {
    yield_().await;
    db.input(x) * 2
}

async fn output_dependencies(db: &mut OwnedAsync<'_>, x: u32) -> u32 {
    db.output(x).await
}

async fn output_transparent(db: &mut OwnedAsync<'_>, x: u32) -> u32 {
    db.output(x).await
}

async fn yield_() {
    let mut yielded = false;
    futures_util::future::poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

#[tokio::test]
async fn basic() {
    let mut query = AsyncDatabase::default();
    query.set_input(22, 23);
    assert_eq!(query.output(22).await, 46);
    assert_eq!(query.output(22).await, 46);
}

fn assert_send<T: Send>(_: T) {}

async fn function(_: &mut AsyncDatabase) {}

#[test]
fn test_send() {
    assert_send(function(&mut AsyncDatabase::default()));
}
