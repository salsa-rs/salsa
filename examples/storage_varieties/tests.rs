#![cfg(test)]

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

#[test]
fn intermingled() {
    let query = QueryContextImpl::default();
    let v1 = query.transparent().of(());
    let v2 = query.memoized().of(());
    let v3 = query.transparent().of(());
    let v4 = query.memoized().of(());

    assert_eq!(v1 + 1, v2);
    assert_eq!(v2 + 1, v3);
    assert_eq!(v2, v4);
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_memoized() {
    let query = QueryContextImpl::default();
    query.cycle_memoized().of(());
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_transparent() {
    let query = QueryContextImpl::default();
    query.cycle_transparent().of(());
}
