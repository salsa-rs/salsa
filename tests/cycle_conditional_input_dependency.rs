//! Test for cycle where only the first iteration of a query depends on the input value.
mod common;

use crate::common::EventLoggerDatabase;
use salsa::{CycleRecoveryAction, Database, Durability, Setter};

#[salsa::input(debug)]
struct Input {
    value: u32,
    max: u32,
}

#[salsa::interned(debug)]
struct Output<'db> {
    #[return_ref]
    value: u32,
}

#[salsa::tracked(cycle_fn=query_a_recover, cycle_initial=query_a_initial)]
fn query_c<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
    query_d(db, input)
}

#[salsa::tracked]
fn query_d<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
    let value = query_c(db, input);
    if value < input.max(db) * 2 {
        // Only the first iteration depends on value but the entire
        // cycle must re-run if input changes.
        let result = value + input.value(db);
        Output::new(db, result);
        result
    } else {
        value
    }
}

// Note: Also requires same output or backdating won't happen.  but other query output needs to be different at least once to fixpint
fn query_a_initial<'db>(db: &'db dyn Database, input: Input) -> u32 {
    0
}

fn query_a_recover<'db>(
    _db: &'db dyn Database,
    _output: &u32,
    _count: u32,
    _input: Input,
) -> CycleRecoveryAction<u32> {
    CycleRecoveryAction::Iterate
}

#[test_log::test]
fn main() {
    #[salsa::tracked(cycle_fn=query_a_recover, cycle_initial=query_a_initial)]
    fn query_a<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
        query_b(db, input)
    }

    #[salsa::tracked]
    fn query_b<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
        let value = query_a(db, input);

        if value < input.max(db) {
            // Only the first iteration depends on value but the entire
            // cycle must re-run if input changes.
            let result = value + input.value(db);
            result
        } else {
            value
        }
    }

    let mut db = EventLoggerDatabase::default();

    let input = Input::builder(4, 5).durability(Durability::MEDIUM).new(&db);

    {
        let result = query_a(&db, input);

        assert_eq!(result, 8);
    }

    {
        input.set_value(&mut db).to(3);

        let result = query_a(&db, input);
        assert_eq!(result, 6);
    }
}

#[test_log::test]
fn nested_cycle_fewer_dependencies_in_first_iteration() {
    #[salsa::interned(debug)]
    struct ClassLiteral<'db> {
        scope: Scope<'db>,
    }

    #[salsa::tracked]
    impl<'db> ClassLiteral<'db> {
        /// Some method on an interned. Panics if any field is accessed before the class literal is re-interned.
        #[salsa::tracked]
        fn context(self, db: &'db dyn salsa::Database) -> u32 {
            // Read a field, that should panic
            let scope = self.scope(db);

            // Access a field on `scope` that changed in the new revision.
            scope.field(db)
        }
    }

    #[salsa::tracked(debug)]
    struct Scope<'db> {
        // #[tracked]
        field: u32,
    }

    /// This query must re-run in the second revision or at least be validated to ensure
    /// the `ClassLiteral` is re-interned.
    #[salsa::tracked]
    fn create_interned<'db>(db: &'db dyn salsa::Database, scope: Scope<'db>) -> ClassLiteral<'db> {
        ClassLiteral::new(db, scope)
    }

    #[derive(Eq, PartialEq, Debug, salsa::Update)]
    struct Index<'db> {
        scope: Scope<'db>,
    }

    #[salsa::tracked(cycle_fn=head_recover, cycle_initial=head_initial)]
    fn cycle_head<'db>(db: &'db dyn salsa::Database, input: Input) -> Option<ClassLiteral<'db>> {
        let b = cycle_outer(db, input);
        tracing::info!("query_b = {b:?}");

        b.or_else(|| {
            let index = index(db, input);
            Some(create_interned(db, index.scope))
        })
    }

    fn head_initial<'db>(_db: &'db dyn Database, _input: Input) -> Option<ClassLiteral<'db>> {
        None
    }

    fn head_recover<'db>(
        _db: &'db dyn Database,
        _output: &Option<ClassLiteral<'db>>,
        _count: u32,
        _input: Input,
    ) -> CycleRecoveryAction<Option<ClassLiteral<'db>>> {
        CycleRecoveryAction::Iterate
    }

    #[salsa::tracked]
    fn cycle_outer<'db>(db: &'db dyn salsa::Database, input: Input) -> Option<ClassLiteral<'db>> {
        // Read some unrelated input that forces the entire cycle to re-executed
        // let _ = input.value(db);
        cycle_participant(db, input)
    }

    #[salsa::tracked]
    fn cycle_participant<'db>(
        db: &'db dyn salsa::Database,
        input: Input,
    ) -> Option<ClassLiteral<'db>> {
        let value = cycle_head(db, input);
        tracing::info!("cycle_head = {value:?}");

        if let Some(value) = value {
            value.context(db);
            Some(value)
        } else {
            None
        }
    }

    #[salsa::tracked(return_ref)]
    fn index<'db>(db: &'db dyn salsa::Database, input: Input) -> Index<'db> {
        Index {
            scope: Scope::new(db, input.value(db) * 2),
        }
    }

    #[salsa::tracked]
    fn entry(db: &dyn salsa::Database, input: Input) -> u32 {
        let _ = input.value(db);
        let head = cycle_head(db, input);

        let participant = cycle_participant(db, input);
        tracing::debug!("head: {head:?}, participant: {participant:?}");

        head.or(participant)
            .map(|class| class.scope(db).field(db))
            .unwrap_or(0)
    }

    let mut db = EventLoggerDatabase::default();

    let input = Input::builder(3, 5)
        .max_durability(Durability::HIGH)
        .value_durability(Durability::LOW)
        .new(&db);

    {
        let result = entry(&db, input);

        assert_eq!(result, 6);
    }

    db.synthetic_write(Durability::LOW);
    db.synthetic_write(Durability::LOW);
    db.synthetic_write(Durability::LOW);
    db.synthetic_write(Durability::MEDIUM);

    {
        input.set_value(&mut db).to(4);
        let result = entry(&db, input);
        assert_eq!(result, 8);
    }
}

// #[test_log::test]
// fn nested_cycle_durability_upgrade() {
//     #[salsa::tracked(cycle_fn=query_a_recover, cycle_initial=query_a_initial)]
//     fn query_a<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
//         let c = query_c(db, input);
//         tracing::debug!("query_c = {}", c);
//         c
//     }

//     // Query b also gets low durability because of query_a. How can we avoid that?
//     // Or is the bug that we loose the durability somehow?
//     #[salsa::tracked]
//     fn query_b<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
//         let value = query_c(db, input);
//         tracing::debug!("query_c = {}", value);

//         value
//     }

//     #[salsa::tracked(cycle_fn=query_a_recover, cycle_initial=query_a_initial)]
//     fn query_c<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
//         let value = query_a(db, input);
//         tracing::debug!("query_a = {}", value);

//         if value < input.max(db) {
//             let b = query_b(db, input);
//             tracing::debug!("query_b = {}", b);
//             Output::new(db, value);
//             input.value(db) + value
//         } else {
//             value
//         }
//     }

//     let mut db = EventLoggerDatabase::default();

//     let input = Input::builder(4, 5).durability(Durability::MEDIUM).new(&db);

//     {
//         let result = query_a(&db, input);

//         assert_eq!(result, 6);
//     }

//     db.synthetic_write(Durability::LOW);
//     db.synthetic_write(Durability::LOW);
//     db.synthetic_write(Durability::LOW);
//     db.synthetic_write(Durability::MEDIUM);

//     {
//         input.set_value(&mut db).to(2);
//         let result = query_a(&db, input);
//         assert_eq!(result, 8);
//     }
// }
