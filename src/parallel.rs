use rayon::iter::{FromParallelIterator, IntoParallelIterator, ParallelIterator};

use crate::{database::RawDatabase, views::DatabaseDownCaster, Database};

pub fn par_map<Db, F, T, R, C>(db: &Db, inputs: impl IntoParallelIterator<Item = T>, op: F) -> C
where
    Db: Database + ?Sized + Send,
    F: Fn(&Db, T) -> R + Sync + Send,
    T: Send,
    R: Send + Sync,
    C: FromParallelIterator<R>,
{
    let views = db.zalsa().views();
    let caster = &views.downcaster_for::<Db>();
    let db_caster = &views.downcaster_for::<dyn Database>();
    inputs
        .into_par_iter()
        .map_with(
            DbForkOnClone(db.fork_db(), caster, db_caster),
            |db, element| op(db.as_view(), element),
        )
        .collect()
}

struct DbForkOnClone<'views, Db: Database + ?Sized>(
    RawDatabase<'static>,
    &'views DatabaseDownCaster<Db>,
    &'views DatabaseDownCaster<dyn Database>,
);

// SAFETY: `T: Send` -> `&own T: Send`, `DbForkOnClone` is an owning pointer
unsafe impl<Db: Send + Database + ?Sized> Send for DbForkOnClone<'_, Db> {}

impl<Db: Database + ?Sized> DbForkOnClone<'_, Db> {
    fn as_view(&self) -> &Db {
        // SAFETY: The downcaster ensures that the pointer is valid for the lifetime of the view.
        unsafe { self.1.downcast_unchecked(self.0) }
    }
}

impl<Db: Database + ?Sized> Drop for DbForkOnClone<'_, Db> {
    fn drop(&mut self) {
        // SAFETY: `caster` is derived from a `db` fitting for our database clone
        let db = unsafe { self.1.downcast_mut_unchecked(self.0) };
        // SAFETY: `db` has been box allocated and leaked by `fork_db`
        _ = unsafe { Box::from_raw(db) };
    }
}

impl<Db: Database + ?Sized> Clone for DbForkOnClone<'_, Db> {
    fn clone(&self) -> Self {
        DbForkOnClone(
            // SAFETY: `caster` is derived from a `db` fitting for our database clone
            unsafe { self.2.downcast_unchecked(self.0) }.fork_db(),
            self.1,
            self.2,
        )
    }
}

pub fn join<A, B, RA, RB, Db: Send + Database + ?Sized>(db: &Db, a: A, b: B) -> (RA, RB)
where
    A: FnOnce(&Db) -> RA + Send,
    B: FnOnce(&Db) -> RB + Send,
    RA: Send,
    RB: Send,
{
    #[derive(Copy, Clone)]
    struct AssertSend<T>(T);
    // SAFETY: We send owning pointers over, which are Send, given the `Db` type parameter above is Send
    unsafe impl<T> Send for AssertSend<T> {}

    let caster = &db.zalsa().views().downcaster_for::<Db>();
    // we need to fork eagerly, as `rayon::join_context` gives us no option to tell whether we get
    // moved to another thread before the closure is executed
    let db_a = AssertSend(db.fork_db());
    let db_b = AssertSend(db.fork_db());
    let res = rayon::join(
        // SAFETY: `caster` is derived from a `db` fitting for our database clone
        move || a(unsafe { caster.downcast_unchecked({ db_a }.0) }),
        // SAFETY: `caster` is derived from a `db` fitting for our database clone
        move || b(unsafe { caster.downcast_unchecked({ db_b }.0) }),
    );

    // SAFETY: `db` has been box allocated and leaked by `fork_db`
    // FIXME: Clean this mess up, RAII
    _ = unsafe { Box::from_raw(caster.downcast_mut_unchecked(db_a.0)) };
    // SAFETY: `db` has been box allocated and leaked by `fork_db`
    _ = unsafe { Box::from_raw(caster.downcast_mut_unchecked(db_b.0)) };
    res
}
