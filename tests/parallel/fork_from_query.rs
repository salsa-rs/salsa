use crate::setup::{ParDatabase, ParDatabaseImpl};

#[test]
#[should_panic]
fn fork_from_query() {
    let db = ParDatabaseImpl::default();
    db.fork_me();
}
