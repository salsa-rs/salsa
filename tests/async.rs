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

    async fn output_inner(&self, x: u32) -> u32;
}

async fn output(db: &mut AsyncDb<'_, (dyn Async + '_)>, x: u32) -> u32 {
    db.output_inner(x).await
}

async fn output_inner(db: &mut AsyncDb<'_, (dyn Async + '_)>, x: u32) -> u32 {
    db.input(x) * 2
}

#[tokio::test]
async fn basic() {
    let mut query = AsyncDatabase::default();
    query.set_input(22, 23);
    assert_eq!(query.output(22).await, 46);
    assert_eq!(query.output(22).await, 46);
}
