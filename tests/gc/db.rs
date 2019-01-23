use crate::group;
use crate::log::{HasLog, Log};

#[derive(Default)]
pub(crate) struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
    log: Log,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

salsa::database_storage! {
    pub(crate) struct DatabaseImplStorage for DatabaseImpl {
        impl group::GcDatabase {
            fn min() for group::MinQuery;
            fn max() for group::MaxQuery;
            fn use_triangular() for group::UseTriangularQuery;
            fn fibonacci() for group::FibonacciQuery;
            fn triangular() for group::TriangularQuery;
            fn compute() for group::ComputeQuery;
            fn compute_all() for group::ComputeAllQuery;
        }
    }
}

impl DatabaseImpl {
    pub(crate) fn clear_log(&self) {
        self.log().take();
    }

    pub(crate) fn assert_log(&self, expected_log: &[&str]) {
        let expected_text = &format!("{:#?}", expected_log);
        let actual_text = &format!("{:#?}", self.log().take());

        if expected_text == actual_text {
            return;
        }

        for diff in diff::lines(expected_text, actual_text) {
            match diff {
                diff::Result::Left(l) => println!("-{}", l),
                diff::Result::Both(l, _) => println!(" {}", l),
                diff::Result::Right(r) => println!("+{}", r),
            }
        }

        panic!("incorrect log results");
    }
}

impl HasLog for DatabaseImpl {
    fn log(&self) -> &Log {
        &self.log
    }
}
