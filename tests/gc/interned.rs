use crate::db;
use salsa::debug::DebugQueryTable;
use salsa::{Database, Durability, InternId, SweepStrategy};

/// Query group for tests for how interned keys interact with GC.
#[salsa::query_group(Intern)]
pub(crate) trait InternDatabase {
    /// A dummy input that can be used to trigger a new revision.
    #[salsa::input]
    fn dummy(&self) -> ();

    /// Underlying interning query.
    #[salsa::interned]
    fn intern_str(&self, x: &'static str) -> InternId;

    /// This just executes the intern query and returns the result.
    fn repeat_intern1(&self, x: &'static str) -> InternId;

    /// Same as `repeat_intern1`. =)
    fn repeat_intern2(&self, x: &'static str) -> InternId;
}

fn repeat_intern1(db: &impl InternDatabase, x: &'static str) -> InternId {
    db.intern_str(x)
}

fn repeat_intern2(db: &impl InternDatabase, x: &'static str) -> InternId {
    db.intern_str(x)
}

/// This test highlights the difference between *interned queries* and
/// other non-input queries -- in particular, their results are not
/// *deterministic*.  Therefore, we cannot GC values that were created
/// in the current revision; that might cause us to re-execute the
/// query twice on the same key during the same revision, which could
/// yield different results each time, wreaking havoc. This test
/// exercises precisely that scenario.
#[test]
fn discard_during_same_revision() {
    let db = db::DatabaseImpl::default();

    // This will assign index 0 for "foo".
    let foo1a = db.repeat_intern1("foo");

    // If we are not careful, this would remove the interned key for
    // "foo".
    db.query(InternStrQuery).sweep(
        SweepStrategy::default()
            .discard_everything()
            .sweep_all_revisions(),
    );

    // This would then reuse index 0 for "bar".
    let bar1 = db.intern_str("bar");

    // And here we would assign index *1* to "foo".
    let foo2 = db.repeat_intern2("foo");

    // But we would still have a cached result, *from the same
    // revision*, with the value 0. So that's inconsistent.
    let foo1b = db.repeat_intern1("foo");

    assert_ne!(foo2, bar1);
    assert_eq!(foo1a, foo1b);
    assert_eq!(foo1b, foo2);
}

/// This test highlights the difference between *interned queries* and
/// other non-input queries -- in particular, their results are not
/// *deterministic*.  Therefore, we cannot GC values that were created
/// in the current revision; that might cause us to re-execute the
/// query twice on the same key during the same revision, which could
/// yield different results each time, wreaking havoc. This test
/// exercises precisely that scenario.
#[test]
fn discard_outdated() {
    let mut db = db::DatabaseImpl::default();

    let foo_from_rev0 = db.repeat_intern1("foo");
    let bar_from_rev0 = db.repeat_intern1("bar");

    // Trigger a new revision.
    db.salsa_runtime_mut().synthetic_write(Durability::HIGH);

    // In this revision, we use "bar".
    let bar_from_rev1 = db.repeat_intern1("bar");

    // This should collect "foo".
    db.sweep_all(SweepStrategy::discard_outdated());

    // This should be the same as before the GC, as bar
    // is not outdated.
    let bar2_from_rev1 = db.repeat_intern1("bar");

    // This should re-use the index of "foo".
    let baz_from_rev1 = db.repeat_intern1("baz");

    // This should assign the next index to "foo".
    let foo_from_rev1 = db.repeat_intern1("foo");

    assert_eq!(bar_from_rev0, bar_from_rev1);
    assert_eq!(bar_from_rev0, bar2_from_rev1);

    assert_eq!(foo_from_rev0, baz_from_rev1);

    assert_ne!(foo_from_rev0, foo_from_rev1);
    assert_ne!(foo_from_rev1, bar_from_rev1);
    assert_ne!(foo_from_rev1, baz_from_rev1);

    assert_eq!(db.lookup_intern_str(foo_from_rev1), "foo");
    assert_eq!(db.lookup_intern_str(bar_from_rev1), "bar");
    assert_eq!(db.lookup_intern_str(baz_from_rev1), "baz");
}

/// Variation on `discard_during_same_revision` --- here we show that
/// a synthetic write of level LOW isn't enough to collect interned
/// keys (which are considered durability HIGH).
#[test]
fn discard_durability_after_synthetic_write_low() {
    let mut db = db::DatabaseImpl::default();

    // This will assign index 0 for "foo".
    let foo1a = db.repeat_intern1("foo");
    assert_eq!(
        Durability::HIGH,
        db.query(RepeatIntern1Query).durability("foo")
    );

    // Trigger a new revision.
    db.salsa_runtime_mut().synthetic_write(Durability::LOW);

    // If we are not careful, this would remove the interned key for
    // "foo".
    db.query(InternStrQuery).sweep(
        SweepStrategy::default()
            .discard_everything()
            .sweep_all_revisions(),
    );

    // This would then reuse index 0 for "bar".
    let bar1 = db.intern_str("bar");

    // And here we would assign index *1* to "foo".
    let foo2 = db.repeat_intern2("foo");

    // But we would still have a cached result with the value 0 and
    // with high durability, so we can reuse it. That gives an
    // inconsistent result.
    let foo1b = db.repeat_intern1("foo");

    assert_ne!(foo2, bar1);
    assert_eq!(foo1a, foo1b);
    assert_eq!(foo1b, foo2);
}

/// Variation on previous test in which we do a synthetic write to
/// `Durability::HIGH`.
#[test]
fn discard_durability_after_synthetic_write_high() {
    let mut db = db::DatabaseImpl::default();

    // This will assign index 0 for "foo".
    let foo1a = db.repeat_intern1("foo");
    assert_eq!(
        Durability::HIGH,
        db.query(RepeatIntern1Query).durability("foo")
    );

    // Trigger a new revision -- marking even high things as having changed.
    db.salsa_runtime_mut().synthetic_write(Durability::HIGH);

    // We are now able to collect "collect".
    db.query(InternStrQuery).sweep(
        SweepStrategy::default()
            .discard_everything()
            .sweep_all_revisions(),
    );

    // So we can reuse index 0 for "bar".
    let bar1 = db.intern_str("bar");

    // And here we assign index *1* to "foo".
    let foo2 = db.repeat_intern2("foo");
    let foo1b = db.repeat_intern1("foo");

    // Thus foo1a (from before the synthetic write) and foo1b (from
    // after) are different.
    assert_ne!(foo1a, foo1b);

    // But the things that come after the synthetic write are
    // consistent.
    assert_ne!(foo2, bar1);
    assert_eq!(foo1b, foo2);
}
