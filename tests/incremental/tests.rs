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

/// Test that:
///
/// - On the first run of R0, we recompute everything.
/// - On the second run of R1, we recompute nothing.
/// - On the first run of R1, we recompute Memoized1 but not Memoized2 (since Memoized1 result
///   did not change).
/// - On the second run of R1, we recompute nothing.
/// - On the first run of R2, we recompute everything (since Memoized1 result *did* change).
#[test]
fn revalidate() {
    env_logger::init();

    let query = QueryContextImpl::default();

    query.memoized2().of(());
    query.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Volatile invoked"]);

    query.memoized2().of(());
    query.assert_log(&[]);

    // Second generation: volatile will change (to 1) but memoized1
    // will not (still 0, as 1/2 = 0)
    query.salsa_runtime().next_revision();

    query.memoized2().of(());
    query.assert_log(&["Memoized1 invoked", "Volatile invoked"]);

    query.memoized2().of(());
    query.assert_log(&[]);

    // Third generation: volatile will change (to 2) and memoized1
    // will too (to 1).  Therefore, after validating that Memoized1
    // changed, we now invoke Memoized2.
    query.salsa_runtime().next_revision();

    query.memoized2().of(());
    query.assert_log(&["Memoized1 invoked", "Volatile invoked", "Memoized2 invoked"]);

    query.memoized2().of(());
    query.assert_log(&[]);
}
