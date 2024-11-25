use std::ops::Deref;

use rayon::iter::{FromParallelIterator, IntoParallelIterator, ParallelIterator};

use crate::Database;

pub fn par_map<Db, D, E, C>(
    db: &Db,
    inputs: impl IntoParallelIterator<Item = D>,
    op: fn(&Db, D) -> E,
) -> C
where
    Db: Database + ?Sized,
    D: Send,
    E: Send + Sync,
    C: FromParallelIterator<E>,
{
    let parallel_db = ParallelDb::Ref(db.as_dyn_database());

    inputs
        .into_par_iter()
        .map_with(parallel_db, |parallel_db, element| {
            let db = parallel_db.as_view::<Db>();
            op(db, element)
        })
        .collect()
}

/// This enum _must not_ be public or used outside of `par_map`.
enum ParallelDb<'db> {
    Ref(&'db dyn Database),
    Fork(Box<dyn Database + Send>),
}

/// SAFETY: the contents of the database are never accessed on the thread
/// where this wrapper type is created.
unsafe impl Send for ParallelDb<'_> {}

impl Deref for ParallelDb<'_> {
    type Target = dyn Database;

    fn deref(&self) -> &Self::Target {
        match self {
            ParallelDb::Ref(db) => *db,
            ParallelDb::Fork(db) => db.as_dyn_database(),
        }
    }
}

impl Clone for ParallelDb<'_> {
    fn clone(&self) -> Self {
        ParallelDb::Fork(self.fork_db())
    }
}
