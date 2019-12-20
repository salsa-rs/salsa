use crate::db;
use salsa::{Database, SweepStrategy};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Query group for tests for how interned keys interact with GC.
#[salsa::query_group(Volatile)]
pub(crate) trait VolatileDatabase: Database {
    #[salsa::input]
    fn atomic_cell(&self) -> Arc<AtomicUsize>;

    /// Underlying volatile query.
    fn volatile(&self) -> usize;

    /// This just executes the intern query and returns the result.
    fn repeat1(&self) -> usize;

    /// Same as `repeat_intern1`. =)
    fn repeat2(&self) -> usize;
}

fn volatile(db: &mut impl VolatileDatabase) -> usize {
    db.salsa_runtime_mut().report_untracked_read();
    db.atomic_cell().load(Ordering::SeqCst)
}

fn repeat1(db: &mut impl VolatileDatabase) -> usize {
    db.volatile()
}

fn repeat2(db: &mut impl VolatileDatabase) -> usize {
    db.volatile()
}

#[test]
fn consistency_no_gc() {
    let mut db = db::DatabaseImpl::default();

    let cell = Arc::new(AtomicUsize::new(22));

    db.set_atomic_cell(cell.clone());

    let v1 = db.repeat1();

    cell.store(23, Ordering::SeqCst);

    let v2 = db.repeat2();

    assert_eq!(v1, v2);
}

#[test]
fn consistency_with_gc() {
    let mut db = db::DatabaseImpl::default();

    let cell = Arc::new(AtomicUsize::new(22));

    db.set_atomic_cell(cell.clone());

    let v1 = db.repeat1();

    cell.store(23, Ordering::SeqCst);
    db.query(VolatileQuery).sweep(
        SweepStrategy::default()
            .discard_everything()
            .sweep_all_revisions(),
    );

    let v2 = db.repeat2();

    assert_eq!(v1, v2);
}
