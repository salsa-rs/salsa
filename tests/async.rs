#![cfg(feature = "async")]
use std::task::Poll;

use salsa::{OwnedDb, ParallelDatabase};

#[salsa::database(async AsyncStorage)]
#[derive(Default)]
struct AsyncDatabase {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for AsyncDatabase {}
impl salsa::ParallelDatabase for AsyncDatabase {
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            storage: self.storage.snapshot(),
        })
    }

    fn fork(&self, forker: salsa::ForkState) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            storage: self.storage.fork(forker),
        })
    }
}

#[salsa::query_group(AsyncStorage)]
trait Async: Send {
    #[salsa::input]
    fn input(&self, x: String) -> u32;

    async fn output(&self, x: String) -> u32;

    async fn query2(&self, x: String) -> u32;

    #[salsa::cycle(recover)]
    async fn output_inner(&self, x: String) -> u32;

    #[salsa::dependencies]
    async fn output_dependencies(&self, x: String) -> u32;

    #[salsa::transparent]
    async fn output_transparent(&self, x: String) -> u32;
}

fn recover<T>(_: &dyn Async, _: &[String], _: &T) -> u32 {
    0
}

async fn output(db: &mut OwnedDb<'_, (dyn Async + '_)>, x: String) -> u32 {
    yield_().await;
    db.output_inner(x).await
}

async fn output_inner(db: &mut OwnedDb<'_, (dyn Async + '_)>, x: String) -> u32 {
    yield_().await;
    db.input(x) * 2
}

async fn output_dependencies(db: &mut OwnedDb<'_, (dyn Async + '_)>, x: String) -> u32 {
    db.output(x).await
}

async fn output_transparent(db: &mut OwnedDb<'_, (dyn Async + '_)>, x: String) -> u32 {
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

async fn query2(db: &mut OwnedDb<'_, dyn Async + '_>, x: String) -> u32 {
    if x == "depends" {
        yield_().await;
        db.query2("base".into()).await + db.input(x)
    } else {
        yield_().await;
        db.input(x)
    }
}

#[tokio::test]
async fn basic() {
    let mut query = AsyncDatabase::default();
    query.set_input("a".into(), 23);
    assert_eq!(query.output("a".into()).await, 46);
    assert_eq!(query.output("a".into()).await, 46);
}

#[tokio::test]
async fn dependency_on_concurrent() {
    let mut query = AsyncDatabase::default();
    query.set_input("base".into(), 2);
    query.set_input("depends".into(), 1);

    let forker = query.forker();
    let mut db1 = forker.fork();
    let mut db2 = forker.fork();
    assert_eq!(
        futures_util::join!(db1.query2("depends".into()), db2.query2("base".into())),
        (3, 2)
    );
}

fn assert_send<T: Send>(_: T) {}

async fn function(_: &mut AsyncDatabase) {}

#[test]
fn test_send() {
    assert_send(function(&mut AsyncDatabase::default()));
}
