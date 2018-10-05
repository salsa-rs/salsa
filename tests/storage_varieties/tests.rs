#![cfg(test)]

use crate::implementation::DatabaseImpl;
use crate::queries::Database;

#[test]
fn memoized_twice() {
    let query = DatabaseImpl::default();
    let v1 = query.memoized(());
    let v2 = query.memoized(());
    assert_eq!(v1, v2);
}

#[test]
fn volatile_twice() {
    let query = DatabaseImpl::default();
    let v1 = query.volatile(());
    let v2 = query.volatile(());
    assert_eq!(v1 + 1, v2);
}

#[test]
fn intermingled() {
    let query = DatabaseImpl::default();
    let v1 = query.volatile(());
    let v2 = query.memoized(());
    let v3 = query.volatile(());
    let v4 = query.memoized(());

    assert_eq!(v1 + 1, v2);
    assert_eq!(v2 + 1, v3);
    assert_eq!(v2, v4);
}
