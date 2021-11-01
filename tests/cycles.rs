use salsa::{Cancelled, ParallelDatabase, Snapshot};
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
// | Intra  | Mixed    | N/A      | Tracked   | direct   | cycle_mixed_1 |
// | Intra  | Mixed    | N/A      | Tracked   | direct   | cycle_mixed_2 |
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

        // Default configuration:
        //
        //     A --> B <-- C
        //     ^     |
        //     +-----+

        res.set_a_invokes(CycleQuery::B);
        res.set_b_invokes(CycleQuery::A);
        res.set_c_invokes(CycleQuery::B);
        res
    }
}

/// The queries A, B, and C in `Database` can be configured
/// to invoke one another in arbitrary ways using this
/// enum.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum CycleQuery {
    None,
    A,
    B,
    C,
    AthenC,
}

#[salsa::query_group(GroupStruct)]
trait Database: salsa::Database {
    // `a` and `b` depend on each other and form a cycle
    fn memoized_a(&self) -> ();
    fn memoized_b(&self) -> ();
    fn volatile_a(&self) -> ();
    fn volatile_b(&self) -> ();

    #[salsa::input]
    fn a_invokes(&self) -> CycleQuery;

    #[salsa::input]
    fn b_invokes(&self) -> CycleQuery;

    #[salsa::input]
    fn c_invokes(&self) -> CycleQuery;

    #[salsa::cycle(recover_a)]
    fn cycle_a(&self) -> Result<(), Error>;

    #[salsa::cycle(recover_b)]
    fn cycle_b(&self) -> Result<(), Error>;

    fn cycle_c(&self) -> Result<(), Error>;
}

fn recover_a(db: &dyn Database, cycle: &salsa::Cycle) -> Result<(), Error> {
    Err(Error {
        cycle: cycle.all_participants(db),
    })
}

fn recover_b(db: &dyn Database, cycle: &salsa::Cycle) -> Result<(), Error> {
    Err(Error {
        cycle: cycle.all_participants(db),
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

impl CycleQuery {
    fn invoke(self, db: &dyn Database) -> Result<(), Error> {
        match self {
            CycleQuery::A => db.cycle_a(),
            CycleQuery::B => db.cycle_b(),
            CycleQuery::C => db.cycle_c(),
            CycleQuery::AthenC => {
                let _ = db.cycle_a();
                db.cycle_c()
            }
            CycleQuery::None => Ok(()),
        }
    }
}

fn cycle_a(db: &dyn Database) -> Result<(), Error> {
    dbg!("cycle_a");
    db.a_invokes().invoke(db)
}

fn cycle_b(db: &dyn Database) -> Result<(), Error> {
    dbg!("cycle_b");
    db.b_invokes().invoke(db)
}

fn cycle_c(db: &dyn Database) -> Result<(), Error> {
    dbg!("cycle_c");
    db.c_invokes().invoke(db)
}

#[test]
fn cycle_memoized() {
    let db = DatabaseImpl::default();
    match Cancelled::catch(|| {
        db.memoized_a();
    }) {
        Err(Cancelled::UnexpectedCycle(c)) => {
            insta::assert_debug_snapshot!(c.unexpected_participants(&db), @r###"
            [
                "memoized_a(())",
                "memoized_b(())",
            ]
            "###);
        }
        v => panic!("unexpected result: {:#?}", v),
    }
}

#[test]
fn cycle_volatile() {
    let db = DatabaseImpl::default();
    match Cancelled::catch(|| {
        db.volatile_a();
    }) {
        Err(Cancelled::UnexpectedCycle(c)) => {
            insta::assert_debug_snapshot!(c.unexpected_participants(&db), @r###"
            [
                "volatile_a(())",
                "volatile_b(())",
            ]
            "###);
        }
        v => panic!("unexpected result: {:#?}", v),
    }
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
    insta::assert_debug_snapshot!(cycle, @r###"
    [
        "cycle_a(())",
        "cycle_b(())",
    ]
    "###);
}

#[test]
fn cycle_revalidate() {
    let mut db = DatabaseImpl::default();
    assert!(db.cycle_a().is_err());
    db.set_b_invokes(CycleQuery::A); // same value as default
    assert!(db.cycle_a().is_err());
}

#[test]
fn cycle_appears() {
    let mut db = DatabaseImpl::default();
    db.set_b_invokes(CycleQuery::None);
    assert!(db.cycle_a().is_ok());
    db.set_b_invokes(CycleQuery::A);
    log::debug!("Set Cycle Leaf");
    assert!(db.cycle_a().is_err());
}

#[test]
fn cycle_disappears() {
    let mut db = DatabaseImpl::default();
    assert!(db.cycle_a().is_err());
    db.set_b_invokes(CycleQuery::None);
    assert!(db.cycle_a().is_ok());
}

#[test]
fn cycle_mixed_1() {
    let mut db = DatabaseImpl::default();
    // Configuration:
    //
    //     A --> B <-- C
    //           |     ^
    //           +-----+
    db.set_b_invokes(CycleQuery::C);
    match Cancelled::catch(|| db.cycle_a()) {
        Err(Cancelled::UnexpectedCycle(u)) => {
            insta::assert_debug_snapshot!((u.all_participants(&db), u.unexpected_participants(&db)), @r###"
            (
                [
                    "cycle_b(())",
                    "cycle_c(())",
                ],
                [
                    "cycle_c(())",
                ],
            )
            "###);
        }
        v => panic!("unexpected result: {:?}", v),
    }
}

#[test]
fn cycle_mixed_2() {
    let mut db = DatabaseImpl::default();
    // Configuration:
    //
    //     A --> B --> C
    //     ^           |
    //     +-----------+
    db.set_b_invokes(CycleQuery::C);
    db.set_c_invokes(CycleQuery::A);
    match Cancelled::catch(|| db.cycle_a()) {
        Err(Cancelled::UnexpectedCycle(u)) => {
            insta::assert_debug_snapshot!((u.all_participants(&db), u.unexpected_participants(&db)), @r###"
            (
                [
                    "cycle_a(())",
                    "cycle_b(())",
                    "cycle_c(())",
                ],
                [
                    "cycle_c(())",
                ],
            )
            "###);
        }
        v => panic!("unexpected result: {:?}", v),
    }
}

#[test]
fn cycle_deterministic_order() {
    // No matter whether we start from A or B, we get the same set of participants:
    let a = DatabaseImpl::default().cycle_a();
    let b = DatabaseImpl::default().cycle_b();
    insta::assert_debug_snapshot!((a, b), @r###"
    (
        Err(
            Error {
                cycle: [
                    "cycle_a(())",
                    "cycle_b(())",
                ],
            },
        ),
        Err(
            Error {
                cycle: [
                    "cycle_a(())",
                    "cycle_b(())",
                ],
            },
        ),
    )
    "###);
}

#[test]
fn cycle_multiple() {
    // No matter whether we start from A or B, we get the same set of participants:
    let mut db = DatabaseImpl::default();

    // Configuration:
    //
    //     A --> B <-- C
    //     ^     |     ^
    //     +-----+     |
    //           |     |
    //           +-----+
    //
    // Here, conceptually, B encounters a cycle with A and then
    // recovers.

    db.set_b_invokes(CycleQuery::AthenC);
    let c = db.cycle_c();
    let b = db.cycle_b();
    let a = db.cycle_a();
    insta::assert_debug_snapshot!((a, b, c), @r###"
    (
        Err(
            Error {
                cycle: [
                    "cycle_a(())",
                    "cycle_b(())",
                ],
            },
        ),
        Err(
            Error {
                cycle: [
                    "cycle_a(())",
                    "cycle_b(())",
                ],
            },
        ),
        Err(
            Error {
                cycle: [
                    "cycle_a(())",
                    "cycle_b(())",
                ],
            },
        ),
    )
    "###);
}
