use crate::db;
use salsa::{Database, SweepStrategy};

/// Query group for tests for how interned keys interact with GC.
#[salsa::query_group(Intern)]
pub(crate) trait InternDatabase {
    /// A dummy input that can be used to trigger a new revision.
    #[salsa::input]
    fn dummy(&self) -> ();

    /// Underlying interning query.
    #[salsa::interned]
    fn intern_str(&self, x: &'static str) -> u32;

    /// This just executes the intern query and returns the result.
    fn repeat_intern1(&self, x: &'static str) -> u32;

    /// Same as `repeat_intern1`. =)
    fn repeat_intern2(&self, x: &'static str) -> u32;
}

fn repeat_intern1(db: &impl InternDatabase, x: &'static str) -> u32 {
    db.intern_str(x)
}

fn repeat_intern2(db: &impl InternDatabase, x: &'static str) -> u32 {
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
    db.set_dummy(());

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
}
