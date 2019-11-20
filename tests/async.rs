use std::sync::Arc;

#[salsa::database(AsyncTraitStorage)]
#[derive(Default)]
struct AsyncDatabase {
    runtime: salsa::Runtime<AsyncDatabase>,
}

impl salsa::Database for AsyncDatabase {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}

impl salsa::ParallelDatabase for AsyncDatabase {
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            runtime: self.runtime.snapshot(self),
        })
    }
    fn fork(&self, forker: Arc<salsa::ForkState<Self>>) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            runtime: self.runtime.fork(self, forker),
        })
    }
}

#[salsa::query_group(AsyncTraitStorage)]
trait AsyncTrait: salsa::ParallelDatabase {
    #[salsa::input]
    fn input(&mut self, x: String) -> u32;

    async fn output(&mut self, x: String) -> u32;
}

async fn output(db: &mut impl AsyncTrait, x: String) -> u32 {
    if x == "a" {
        let forker = db.forker();
        let mut db1 = forker.fork();
        let mut db2 = forker.fork();
        let (b, c) = futures::join!(db1.output("b".into()), db2.output("c".into()));
        b + c
    } else {
        db.input(x)
    }
}

#[test]
fn basic() {
    let mut query = AsyncDatabase::default();
    query.set_input("b".into(), 2);
    query.set_input("c".into(), 3);
    assert_eq!(futures::executor::block_on(query.output("a".into())), 2 + 3);
}

fn assert_send<T: Send>(t: T) -> T {
    t
}

async fn function(_: &mut AsyncDatabase) {}

#[test]
fn test_send() {
    assert_send(function(&mut AsyncDatabase::default()));
}
