use crate::implementation::QueryContextImpl;
use crate::queries::QueryContext;

#[test]
fn memoized_twice() {
    let query = QueryContextImpl::default();
    let v1 = query.memoized().of(());
    let v2 = query.memoized().of(());
    assert_eq!(v1, v2);
}

#[test]
fn transparent_twice() {
    let query = QueryContextImpl::default();
    let v1 = query.transparent().of(());
    let v2 = query.transparent().of(());
    assert_eq!(v1 + 1, v2);
}
