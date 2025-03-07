use rayon::iter::{FromParallelIterator, IntoParallelIterator, ParallelIterator};

use crate::Database;

pub fn par_map<Db, F, T, R, C>(db: &Db, inputs: impl IntoParallelIterator<Item = T>, op: F) -> C
where
    Db: Database + ?Sized,
    F: Fn(&Db, T) -> R + Sync + Send,
    T: Send,
    R: Send + Sync,
    C: FromParallelIterator<R>,
{
    let parallel_db = ParallelDb::Ref(db.as_dyn_database());

    inputs
        .into_par_iter()
        .map_with(parallel_db, |parallel_db, element| {
            let db = match parallel_db {
                ParallelDb::Ref(db) => db.as_view::<Db>(),
                ParallelDb::Fork(db) => db.as_view::<Db>(),
            };
            op(db, element)
        })
        .collect()
}

/// This enum _must not_ be public or used outside of `par_map`.
enum ParallelDb<'db> {
    Ref(&'db dyn Database),
    Fork(Box<dyn Database>),
}

/// SAFETY: We guarantee that the `&'db dyn Database` reference is not copied and as such it is
/// never referenced on multiple threads at once.
unsafe impl Send for ParallelDb<'_> where dyn Database: Send {}

impl Clone for ParallelDb<'_> {
    fn clone(&self) -> Self {
        ParallelDb::Fork(match self {
            ParallelDb::Ref(db) => db.fork_db(),
            ParallelDb::Fork(db) => db.fork_db(),
        })
    }
}
