//! Tests that `force_invalidation_on_cache_eviction` causes dependent queries
//! to be recomputed if our memo is missing, even if our result is unchanged.

mod common;
use common::LogDatabase;
use expect_test::expect;
use salsa::{Durability, Setter};
use test_log::test;

#[salsa::input]
struct MyInput {
    field: (),
}

#[salsa::tracked(force_invalidation_on_cache_eviction, lru = 1)]
fn intermediate_result(db: &dyn LogDatabase, input: MyInput) {
    db.push_log(format!("intermediate_result({})", input.0.as_u32()));
    input.field(db);
}

#[salsa::tracked]
fn final_result(db: &dyn LogDatabase, input: MyInput) {
    db.push_log("final_result".to_string());
    intermediate_result(db, input);
}

#[test]
fn execute() {
    let mut db = common::LoggerDatabase::default();

    let high = MyInput::builder(()).durability(Durability::HIGH).new(&db);
    let low = MyInput::new(&db, ());

    final_result(&db, high);
    // on first run, both intermediate and final results were computed
    db.assert_logs(expect![[r#"
        [
            "final_result",
            "intermediate_result(0)",
        ]"#]]);

    // an intermediate result for a different input will evict the original memo
    // from the cache (because query's lru = 1) when revision is bumped
    intermediate_result(&db, low);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(1)",
        ]"#]]);

    // bump the revision, causing cache eviction
    low.set_field(&mut db).to(());

    final_result(&db, high);
    // now, despite unchanged intermediate result, final result was recomputed
    // (because intermediate query is `force_invalidation_on_cache_eviction`)
    db.assert_logs(expect![[r#"
        [
            "final_result",
            "intermediate_result(0)",
        ]"#]]);
}
