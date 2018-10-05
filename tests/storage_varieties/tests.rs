#![cfg(test)]

use crate::implementation::DatabaseImpl;
use crate::queries::Database;

#[test]
fn memoized_twice() {
    let query = DatabaseImpl::default();
    let v1 = query.memoized().read();
    let v2 = query.memoized().read();
    assert_eq!(v1, v2);
}

#[test]
fn volatile_twice() {
    let query = DatabaseImpl::default();
    let v1 = query.volatile().read();
    let v2 = query.volatile().read();
    assert_eq!(v1 + 1, v2);
}

#[test]
fn intermingled() {
    let query = DatabaseImpl::default();
    let v1 = query.volatile().read();
    let v2 = query.memoized().read();
    let v3 = query.volatile().read();
    let v4 = query.memoized().read();

    assert_eq!(v1 + 1, v2);
    assert_eq!(v2 + 1, v3);
    assert_eq!(v2, v4);
}
