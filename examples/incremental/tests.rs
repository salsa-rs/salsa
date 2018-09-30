#![cfg(test)]

use crate::implementation::QueryContextImpl;
use crate::queries::CounterContext;
use crate::queries::QueryContext;

impl QueryContextImpl {
    fn assert_log(&self, expected_log: &[&str]) {
        let expected_text = format!("{:#?}", expected_log);
        let actual_text = format!("{:#?}", self.log().read());
        text_diff::assert_diff(&expected_text, &actual_text, "", 0);
    }
}

#[test]
fn foo() {
    let query = QueryContextImpl::default();

    // Invoking volatile twice will simply execute twice.
    query.volatile().of(());
    query.volatile().of(());
    query.assert_log(&["Volatile invoked", "Volatile invoked"]);
}
