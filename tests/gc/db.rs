use crate::group;

#[derive(Default)]
pub struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

salsa::database_storage! {
    pub struct DatabaseImplStorage for DatabaseImpl {
        impl group::GcDatabase {
            fn min() for group::Min;
            fn max() for group::Max;
            fn use_triangular() for group::UseTriangular;
            fn fibonacci() for group::Fibonacci;
            fn triangular() for group::Triangular;
            fn compute() for group::Compute;
            fn compute_all() for group::ComputeAll;
        }
    }
}
