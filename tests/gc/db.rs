use crate::group;
use crate::interned;
use crate::log::{HasLog, Log};
use crate::volatile_tests;

#[salsa::database(group::Gc, interned::Intern, volatile_tests::Volatile)]
#[derive(Default)]
pub(crate) struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
    log: Log,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<DatabaseImpl> {
        &mut self.runtime
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
