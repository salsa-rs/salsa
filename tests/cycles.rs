use salsa::{ParallelDatabase, Snapshot};
use test_env_log::test;

// Axes:
//
// Threading
// * Intra-thread
// * Cross-thread -- part of cycle is on one thread, part on another
//
// Recovery strategies:
// * Panic
// * Fallback
// * Mixed -- multiple strategies within cycle participants
//
// Across revisions:
// * N/A -- only one revision
// * Present in new revision, not old
// * Present in old revision, not new
// * Present in both revisions
//
// Dependencies
// * Tracked
// * Untracked -- cycle participant(s) contain untracked reads
//
// Layers
// * Direct -- cycle participant is directly invoked from test
// * Indirect -- invoked a query that invokes the cycle
//
//
// | Thread | Recovery | Old, New | Dep style | Layers   | Test Name      |
// | ------ | -------- | -------- | --------- | ------   | ---------      |
// | Intra  | Panic    | N/A      | Tracked   | direct   | cycle_memoized |
// | Intra  | Panic    | N/A      | Untracked | direct   | cycle_volatile |
// | Intra  | Fallback | N/A      | Tracked   | direct   | cycle_cycle  |
// | Intra  | Fallback | N/A      | Tracked   | indirect | inner_cycle |
// | Intra  | Fallback | Both     | Tracked   | direct   | cycle_revalidate |
// | Intra  | Fallback | New      | Tracked   | direct   | cycle_appears |
// | Intra  | Fallback | Old      | Tracked   | direct   | cycle_disappears |
// | Cross  | Fallback | N/A      | Tracked   | both     | parallel/cycles.rs: recover_parallel_cycle |
// | Cross  | Panic    | N/A      | Tracked   | both     | parallel/cycles.rs: panic_parallel_cycle |

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct Error {
    cycle: Vec<String>,
}

#[salsa::database(GroupStruct)]
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
}

impl Default for DatabaseImpl {
    fn default() -> Self {
        let mut res = DatabaseImpl {
            storage: salsa::Storage::default(),
        };
        res.set_should_create_cycle(true);
        res
    }
}

#[salsa::query_group(GroupStruct)]
trait Database: salsa::Database {
    // `a` and `b` depend on each other and form a cycle
    fn memoized_a(&self) -> ();
    fn memoized_b(&self) -> ();
    fn volatile_a(&self) -> ();
    fn volatile_b(&self) -> ();

    #[salsa::input]
    fn should_create_cycle(&self) -> bool;

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

fn memoized_a(db: &dyn Database) {
    db.memoized_b()
}

fn memoized_b(db: &dyn Database) {
    db.memoized_a()
}

fn volatile_a(db: &dyn Database) {
    db.salsa_runtime().report_untracked_read();
    db.volatile_b()
}

fn volatile_b(db: &dyn Database) {
    db.salsa_runtime().report_untracked_read();
    db.volatile_a()
}

fn cycle_a(db: &dyn Database) -> Result<(), Error> {
    let _ = db.cycle_b();
    Ok(())
}

fn cycle_b(db: &dyn Database) -> Result<(), Error> {
    if db.should_create_cycle() {
        let _ = db.cycle_a();
    }
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
fn cycle_revalidate() {
    let mut db = DatabaseImpl::default();
    assert!(db.cycle_a().is_err());
    db.set_should_create_cycle(true);
    assert!(db.cycle_a().is_err());
}

#[test]
fn cycle_appears() {
    let mut db = DatabaseImpl::default();
    db.set_should_create_cycle(false);
    assert!(db.cycle_a().is_ok());
    db.set_should_create_cycle(true);
    log::debug!("Set Cycle Leaf");
    assert!(db.cycle_a().is_err());
}

#[test]
fn cycle_disappears() {
    let mut db = DatabaseImpl::default();
    assert!(db.cycle_a().is_err());
    db.set_should_create_cycle(false);
    assert!(db.cycle_a().is_ok());
}
