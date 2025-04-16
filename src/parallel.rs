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
    inputs
        .into_par_iter()
        .map_with(DbForkOnClone(db.fork_db()), |db, element| {
            op(db.0.as_view(), element)
        })
        .collect()
}

struct DbForkOnClone(Box<dyn Database>);

impl Clone for DbForkOnClone {
    fn clone(&self) -> Self {
        DbForkOnClone(self.0.fork_db())
    }
}

pub fn join<A, B, RA, RB, Db: Database + ?Sized>(db: &Db, a: A, b: B) -> (RA, RB)
where
    A: FnOnce(&Db) -> RA + Send,
    B: FnOnce(&Db) -> RB + Send,
    RA: Send,
    RB: Send,
{
    // we need to fork eagerly, as `rayon::join_context` gives us no option to tell whether we get
    // moved to another thread before the closure is executed
    let db_a = db.fork_db();
    let db_b = db.fork_db();
    rayon::join(
        move || a(db_a.as_view::<Db>()),
        move || b(db_b.as_view::<Db>()),
    )
}
