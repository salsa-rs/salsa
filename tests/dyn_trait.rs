//! Test that you can implement a query using a `dyn Trait` setup.

#[salsa::database(DynTraitStorage)]
#[derive(Default)]
struct DynTraitDatabase {
    runtime: salsa::Runtime<DynTraitDatabase>,
}

impl salsa::Database for DynTraitDatabase {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }
}

#[salsa::query_group(DynTraitStorage)]
trait DynTrait {
    #[salsa::input]
    fn input(&self, x: u32) -> u32;

    fn output(&self, x: u32) -> u32;
}

fn output(db: &mut dyn DynTrait, x: u32) -> u32 {
    db.input(x) * 2
}

#[test]
fn dyn_trait() {
    let mut query = DynTraitDatabase::default();
    query.set_input(22, 23);
    assert_eq!(query.output(22), 46);
}
