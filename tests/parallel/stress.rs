use rand::Rng;
use salsa::Database;
use salsa::Frozen;
use salsa::ParallelDatabase;
use salsa::SweepStrategy;

// Number of operations a reader performs
const N_MUTATOR_OPS: usize = 100;
const N_READER_OPS: usize = 100;

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
    fn fork(&self) -> Frozen<StressDatabaseImpl> {
        Frozen::new(StressDatabaseImpl {
            runtime: self.runtime.fork(self),
        })
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

enum MutatorOp {
    WriteOp(WriteOp),
    LaunchReader {
        ops: Vec<ReadOp>,
        check_cancellation: bool,
    },
}

#[derive(Debug)]
enum WriteOp {
    SetA(usize, usize),
}

#[derive(Debug)]
enum ReadOp {
    Get(Query, usize),
    Gc(Query, SweepStrategy),
    GcAll(SweepStrategy),
}

impl rand::distributions::Distribution<Query> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> Query {
        *rng.choose(&[Query::A, Query::B, Query::C]).unwrap()
    }
}

impl rand::distributions::Distribution<MutatorOp> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> MutatorOp {
        if rng.gen_bool(0.5) {
            MutatorOp::WriteOp(rng.gen())
        } else {
            MutatorOp::LaunchReader {
                ops: (0..N_READER_OPS).map(|_| rng.gen()).collect(),
                check_cancellation: rng.gen(),
            }
        }
    }
}

impl rand::distributions::Distribution<WriteOp> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> WriteOp {
        let key = rng.gen::<usize>() % 10;
        let value = rng.gen::<usize>() % 10;
        return WriteOp::SetA(key, value);
    }
}

impl rand::distributions::Distribution<ReadOp> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> ReadOp {
        if rng.gen_bool(0.5) {
            let query = rng.gen::<Query>();
            let key = rng.gen::<usize>() % 10;
            return ReadOp::Get(query, key);
        }
        let mut strategy = SweepStrategy::default();
        if rng.gen_bool(0.5) {
            strategy = strategy.discard_values();
        }
        if rng.gen_bool(0.5) {
            ReadOp::Gc(rng.gen::<Query>(), strategy)
        } else {
            ReadOp::GcAll(strategy)
        }
    }
}

fn db_reader_thread(db: &StressDatabaseImpl, ops: Vec<ReadOp>, check_cancellation: bool) {
    for op in ops {
        if check_cancellation {
            if db.salsa_runtime().is_current_revision_canceled() {
                return;
            }
        }
        op.execute(db);
    }
}

impl WriteOp {
    fn execute(self, db: &mut StressDatabaseImpl) {
        match self {
            WriteOp::SetA(key, value) => {
                db.query(A).set(key, value);
            }
        }
    }
}

impl ReadOp {
    fn execute(self, db: &StressDatabaseImpl) {
        match self {
            ReadOp::Get(query, key) => match query {
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
            ReadOp::Gc(query, strategy) => match query {
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
            ReadOp::GcAll(strategy) => {
                db.sweep_all(strategy);
            }
        }
    }
}

#[test]
fn stress_test() {
    let mut db = StressDatabaseImpl::default();
    for i in 0..10 {
        db.query(A).set(i, i);
    }

    let mut rng = rand::thread_rng();

    // generate the ops that the mutator thread will perform
    let write_ops: Vec<MutatorOp> = (0..N_MUTATOR_OPS).map(|_| rng.gen()).collect();

    // execute the "main thread", which sometimes forks off other threads
    let mut all_threads = vec![];
    for op in write_ops {
        match op {
            MutatorOp::WriteOp(w) => w.execute(&mut db),
            MutatorOp::LaunchReader {
                ops,
                check_cancellation,
            } => all_threads.push(std::thread::spawn({
                let db = db.fork();
                move || db_reader_thread(&db, ops, check_cancellation)
            })),
        }
    }

    for thread in all_threads {
        thread.join().unwrap();
    }
}
