#![cfg(test)]

use crate::implementation::QueryContextImpl;
use crate::queries::CounterContext;
use crate::queries::QueryContext as _;
use salsa::QueryContext as _;

impl QueryContextImpl {
    fn assert_log(&self, expected_log: &[&str]) {
        use difference::{Changeset, Difference};

        let expected_text = &format!("{:#?}", expected_log);
        let actual_text = &format!("{:#?}", self.log().take());

        if expected_text == actual_text {
            return;
        }

        let Changeset { diffs, .. } = Changeset::new(expected_text, actual_text, "\n");

        for i in 0..diffs.len() {
            match &diffs[i] {
                Difference::Same(x) => println!(" {}", x),
                Difference::Add(x) => println!("+{}", x),
                Difference::Rem(x) => println!("-{}", x),
            }
        }

        panic!("incorrect log results");
    }
}

#[test]
fn volatile_x2() {
    let query = QueryContextImpl::default();

    // Invoking volatile twice will simply execute twice.
    query.volatile().of(());
    query.volatile().of(());
    query.assert_log(&["Volatile invoked", "Volatile invoked"]);
}

#[test]
fn foo() {
    env_logger::init();

    let query = QueryContextImpl::default();

    query.memoized2().of(());
    query.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Volatile invoked"]);

    query.memoized2().of(());
    query.assert_log(&[]);

    query.salsa_runtime().next_revision();

    query.memoized2().of(());
    query.assert_log(&["Memoized1 invoked", "Volatile invoked"]);
}
