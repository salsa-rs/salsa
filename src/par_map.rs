use std::thread::{self, ThreadId};

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
    let parallel_db = ParallelDb::Ref(
        db.as_dyn_database(),
        #[cfg(debug_assertions)]
        thread::current().id(),
    );

    inputs
        .into_par_iter()
        .map_with(parallel_db, |parallel_db, element| {
            op(parallel_db.as_view(), element)
        })
        .collect()
}

/// This enum _must not_ be public or used outside of `par_map`.
enum ParallelDb<'db> {
    Ref(
        &'db dyn Database,
        #[cfg(debug_assertions)] ThreadId,
        #[cfg(not(debug_assertions))] (),
    ),
    Fork(Box<dyn Database>),
}

/// SAFETY: We guarantee that the `&'db dyn Database` reference is not copied and as such it is
/// never referenced on multiple threads at once.
unsafe impl Send for ParallelDb<'_> where dyn Database: Send {}

impl ParallelDb<'_> {
    #[inline]
    #[track_caller]
    fn thread_id_assert(&self) {
        #[cfg(debug_assertions)]
        {
            match self {
                ParallelDb::Ref(_, thread_id) => {
                    assert_eq!(
                        thread_id,
                        &thread::current().id(),
                        "reference was smuggled to another thread!"
                    );
                }
                ParallelDb::Fork(_) => {}
            }
        }
    }

    fn fork(&self) -> ParallelDb<'static> {
        self.thread_id_assert();
        ParallelDb::Fork(match self {
            ParallelDb::Ref(db, _) => db.fork_db(),
            ParallelDb::Fork(db) => db.fork_db(),
        })
    }

    fn as_view<Db: Database + ?Sized>(&self) -> &Db {
        self.thread_id_assert();
        match self {
            ParallelDb::Ref(db, _) => db.as_view::<Db>(),
            ParallelDb::Fork(db) => db.as_view::<Db>(),
        }
    }
}

impl Clone for ParallelDb<'_> {
    fn clone(&self) -> Self {
        self.thread_id_assert();
        ParallelDb::Fork(match self {
            ParallelDb::Ref(db, _) => db.fork_db(),
            ParallelDb::Fork(db) => db.fork_db(),
        })
    }
}

pub struct Scope<'scope, 'local, Db: Database + ?Sized> {
    db: ParallelDb<'local>,
    base: &'local rayon::Scope<'scope>,
    phantom: std::marker::PhantomData<fn() -> Db>,
}

impl<'scope, Db: Database + ?Sized> Scope<'scope, '_, Db> {
    pub fn spawn<BODY>(&self, body: BODY)
    where
        BODY: for<'l> FnOnce(&'l Scope<'scope, 'l, Db>, &Db) + Send + 'scope,
    {
        let db = self.db.fork();
        self.base.spawn(move |scope| {
            let scope = Scope {
                db,
                base: scope,
                phantom: std::marker::PhantomData,
            };
            body(&scope, scope.db.as_view::<Db>())
        })
    }
}

pub fn scope<'scope, Db: Database + ?Sized, OP, R>(db: &Db, op: OP) -> R
where
    OP: FnOnce(&Scope<'scope, '_, Db>, &Db) -> R + Send,
    R: Send,
{
    rayon::in_place_scope(move |s| {
        let scope = Scope {
            db: ParallelDb::Ref(
                db.as_dyn_database(),
                #[cfg(debug_assertions)]
                thread::current().id(),
                #[cfg(not(debug_assertions))]
                (),
            ),
            base: s,
            phantom: std::marker::PhantomData,
        };
        op(&scope, db)
    })
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
