use salsa::{ParallelDatabase, Snapshot};

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct Error {
    cycle: Vec<String>,
}

#[salsa::database(GroupStruct)]
#[derive(Default)]
struct DatabaseImpl {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for DatabaseImpl {}

impl ParallelDatabase for DatabaseImpl {
    fn snapshot(&self) -> Snapshot<Self> {
        Snapshot::new(DatabaseImpl {
            storage: self.storage.snapshot(),
        })
    }

    fn fork(&self, forker: salsa::ForkState) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            storage: self.storage.fork(forker),
        })
    }
}

#[salsa::query_group(GroupStruct)]
trait Database: salsa::Database {
    // `a` and `b` depend on each other and form a cycle
    fn memoized_a(&self) -> ();
    fn memoized_b(&self) -> ();
    fn volatile_a(&self) -> ();
    fn volatile_b(&self) -> ();

    fn cycle_leaf(&self) -> ();

    #[salsa::cycle(recover_a)]
    fn cycle_a(&self) -> Result<(), Error>;
    #[salsa::cycle(recover_b)]
    fn cycle_b(&self) -> Result<(), Error>;

    fn cycle_c(&self) -> Result<(), Error>;
}

fn recover_a(_db: &dyn Database, cycle: &[String]) -> Result<(), Error> {
    Err(Error {
        cycle: cycle.to_owned(),
    })
}

fn recover_b(_db: &dyn Database, cycle: &[String]) -> Result<(), Error> {
    Err(Error {
        cycle: cycle.to_owned(),
    })
}

fn memoized_a(db: &dyn Database) -> () {
    db.memoized_b()
}

fn memoized_b(db: &dyn Database) -> () {
    db.memoized_a()
}

fn volatile_a(db: &dyn Database) -> () {
    db.salsa_runtime().report_untracked_read();
    db.volatile_b()
}

fn volatile_b(db: &dyn Database) -> () {
    db.salsa_runtime().report_untracked_read();
    db.volatile_a()
}

fn cycle_leaf(_db: &dyn Database) -> () {}

fn cycle_a(db: &dyn Database) -> Result<(), Error> {
    let _ = db.cycle_b();
    Ok(())
}

fn cycle_b(db: &dyn Database) -> Result<(), Error> {
    db.cycle_leaf();
    let _ = db.cycle_a();
    Ok(())
}

fn cycle_c(db: &dyn Database) -> Result<(), Error> {
    db.cycle_b()
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_memoized() {
    let query = DatabaseImpl::default();
    query.memoized_a();
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_volatile() {
    let query = DatabaseImpl::default();
    query.volatile_a();
}

#[test]
fn cycle_cycle() {
    let query = DatabaseImpl::default();
    assert!(query.cycle_a().is_err());
}

#[test]
fn inner_cycle() {
    let query = DatabaseImpl::default();
    let err = query.cycle_c();
    assert!(err.is_err());
    let cycle = err.unwrap_err().cycle;
    assert!(
        cycle
            .iter()
            .zip(&["cycle_b", "cycle_a"])
            .all(|(l, r)| l.contains(r)),
        "{:#?}",
        cycle
    );
}

#[test]
fn parallel_cycle() {
    let _ = env_logger::try_init();

    let db = DatabaseImpl::default();
    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            let result = db.cycle_a();
            assert!(result.is_err(), "Expected cycle error");
            let cycle = result.unwrap_err().cycle;
            assert!(
                cycle
                    .iter()
                    .all(|l| ["cycle_b", "cycle_a"].iter().any(|r| l.contains(r))),
                "{:#?}",
                cycle
            );
        }
    });

    let thread2 = std::thread::spawn(move || {
        let result = db.cycle_c();
        assert!(result.is_err(), "Expected cycle error");
        let cycle = result.unwrap_err().cycle;
        assert!(
            cycle
                .iter()
                .all(|l| ["cycle_b", "cycle_a"].iter().any(|r| l.contains(r))),
            "{:#?}",
            cycle
        );
    });

    thread1.join().unwrap();
    thread2.join().unwrap();
    eprintln!("OK");
}
