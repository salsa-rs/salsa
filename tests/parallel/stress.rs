use rand::Rng;

use salsa::Database;
use salsa::ParallelDatabase;
use salsa::SweepStrategy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Canceled;
type Cancelable<T> = Result<T, Canceled>;

salsa::query_group! {
    trait StressDatabase: salsa::Database {
        fn a(key: usize) -> usize {
            type A;
            storage input;
        }

        fn b(key: usize) -> Cancelable<usize> {
            type B;
        }

        fn c(key: usize) -> Cancelable<usize> {
            type C;
        }
    }
}

fn b(db: &impl StressDatabase, key: usize) -> Cancelable<usize> {
    if db.salsa_runtime().is_current_revision_canceled() {
        return Err(Canceled);
    }
    Ok(db.a(key))
}

fn c(db: &impl StressDatabase, key: usize) -> Cancelable<usize> {
    db.b(key)
}

#[derive(Default)]
struct StressDatabaseImpl {
    runtime: salsa::Runtime<StressDatabaseImpl>,
}

impl salsa::Database for StressDatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<StressDatabaseImpl> {
        &self.runtime
    }
}

impl salsa::ParallelDatabase for StressDatabaseImpl {
    fn fork_mut(&self) -> StressDatabaseImpl {
        StressDatabaseImpl {
            runtime: self.runtime.fork_mut(),
        }
    }
}

salsa::database_storage! {
    pub struct DatabaseImplStorage for StressDatabaseImpl {
        impl StressDatabase {
            fn a() for A;
            fn b() for B;
            fn c() for C;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Query {
    A,
    B,
    C,
}

#[derive(Debug)]
enum Op {
    SetA(usize, usize),
    Get(Query, usize),
    Gc(Query, SweepStrategy),
    GcAll(SweepStrategy),
}

impl rand::distributions::Distribution<Query> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> Query {
        *rng.choose(&[Query::A, Query::B, Query::C]).unwrap()
    }
}

impl rand::distributions::Distribution<Op> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> Op {
        if rng.gen_bool(0.5) {
            let query = rng.gen::<Query>();
            let key = rng.gen::<usize>() % 10;
            return Op::Get(query, key);
        }
        if rng.gen_bool(0.5) {
            let key = rng.gen::<usize>() % 10;
            let value = rng.gen::<usize>() % 10;
            return Op::SetA(key, value);
        }
        let mut strategy = SweepStrategy::default();
        if rng.gen_bool(0.5) {
            strategy = strategy.discard_values();
        }
        if rng.gen_bool(0.5) {
            Op::Gc(rng.gen::<Query>(), strategy)
        } else {
            Op::GcAll(strategy)
        }
    }
}

fn db_thread(db: StressDatabaseImpl, ops: Vec<Op>) {
    for op in ops {
        // eprintln!("{:02?}: {:?}", std::thread::current().id(), op);
        match op {
            Op::SetA(key, value) => {
                db.query(A).set(key, value);
            }
            Op::Get(query, key) => match query {
                Query::A => {
                    db.a(key);
                }
                Query::B => {
                    let _ = db.b(key);
                }
                Query::C => {
                    let _ = db.c(key);
                }
            },
            Op::Gc(query, strategy) => match query {
                Query::A => {
                    db.query(A).sweep(strategy);
                }
                Query::B => {
                    db.query(B).sweep(strategy);
                }
                Query::C => {
                    db.query(C).sweep(strategy);
                }
            },
            Op::GcAll(strategy) => {
                db.sweep_all(strategy);
            }
        }
    }
}

fn random_ops(n_ops: usize) -> Vec<Op> {
    let mut rng = rand::thread_rng();
    (0..n_ops).map(|_| rng.gen::<Op>()).collect()
}

#[test]
fn stress_test() {
    let db = StressDatabaseImpl::default();
    for i in 0..10 {
        db.query(A).set(i, i);
    }
    let n_threads = 20;
    let n_ops = 100;
    let ops = (0..n_threads).map(|_| random_ops(n_ops));
    let threads = ops
        .into_iter()
        .map(|ops| {
            let db = db.fork_mut();
            std::thread::spawn(move || db_thread(db, ops))
        })
        .collect::<Vec<_>>();
    std::mem::drop(db);
    for thread in threads {
        thread.join().unwrap();
    }
}
