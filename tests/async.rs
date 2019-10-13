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

#[salsa::query_group(AsyncTraitStorage)]
trait AsyncTrait {
    #[salsa::input]
    fn input(&self, x: String) -> u32;

    async fn output(&self, x: String) -> u32;
}

async fn output(db: &impl AsyncTrait, x: String) -> u32 {
    if x == "a" {
        let (b, c) = futures::join!(db.output("b".into()), db.output("c".into()));
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
