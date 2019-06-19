#[salsa::database(GroupStruct)]
#[derive(Default)]
struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

#[salsa::query_group(GroupStruct)]
trait Database: salsa::Database {
    // `a` and `b` depend on each other and form a cycle
    fn memoized_a(&self) -> ();
    fn memoized_b(&self) -> ();
    fn volatile_a(&self) -> ();
    fn volatile_b(&self) -> ();
}

fn memoized_a(db: &impl Database) -> () {
    db.memoized_b()
}

fn memoized_b(db: &impl Database) -> () {
    db.memoized_a()
}

fn volatile_a(db: &impl Database) -> () {
    db.salsa_runtime().report_untracked_read();
    db.volatile_b()
}

fn volatile_b(db: &impl Database) -> () {
    db.salsa_runtime().report_untracked_read();
    db.volatile_a()
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
