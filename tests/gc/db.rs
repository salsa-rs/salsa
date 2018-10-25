use crate::group;
use crate::log::{HasLog, Log};

#[derive(Default)]
pub struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
    log: Log,
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
