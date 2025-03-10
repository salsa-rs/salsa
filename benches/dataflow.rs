//! Benchmark for fixpoint iteration cycle resolution.
//!
//! This benchmark simulates a (very simplified) version of a real dataflow analysis using fixpoint
//! iteration.
use codspeed_criterion_compat::{criterion_group, criterion_main, BatchSize, Criterion};
use salsa::{CycleRecoveryAction, Database as Db, Setter};
use std::collections::BTreeSet;
use std::iter::IntoIterator;

/// A Use of a symbol.
#[salsa::input]
struct Use {
    reaching_definitions: Vec<Definition>,
}

/// A Definition of a symbol, either of the form `base + increment` or `0 + increment`.
#[salsa::input]
struct Definition {
    base: Option<Use>,
    increment: usize,
}

#[derive(Eq, PartialEq, Clone, Debug, salsa::Update)]
enum Type {
    Bottom,
    Values(Box<[usize]>),
    Top,
}

impl Type {
    fn join(tys: impl IntoIterator<Item = Type>) -> Type {
        let mut result = Type::Bottom;
        for ty in tys.into_iter() {
            result = match (result, ty) {
                (result, Type::Bottom) => result,
                (_, Type::Top) => Type::Top,
                (Type::Top, _) => Type::Top,
                (Type::Bottom, ty) => ty,
                (Type::Values(a_ints), Type::Values(b_ints)) => {
                    let mut set = BTreeSet::new();
                    set.extend(a_ints);
                    set.extend(b_ints);
                    Type::Values(set.into_iter().collect())
                }
            }
        }
        result
    }
}

#[salsa::tracked(cycle_fn=use_cycle_recover, cycle_initial=use_cycle_initial)]
fn infer_use<'db>(db: &'db dyn Db, u: Use) -> Type {
    let defs = u.reaching_definitions(db);
    match defs[..] {
        [] => Type::Bottom,
        [def] => infer_definition(db, def),
        _ => Type::join(defs.iter().map(|&def| infer_definition(db, def))),
    }
}

#[salsa::tracked(cycle_fn=def_cycle_recover, cycle_initial=def_cycle_initial)]
fn infer_definition<'db>(db: &'db dyn Db, def: Definition) -> Type {
    let increment_ty = Type::Values(Box::from([def.increment(db)]));
    if let Some(base) = def.base(db) {
        let base_ty = infer_use(db, base);
        add(&base_ty, &increment_ty)
    } else {
        increment_ty
    }
}

fn def_cycle_initial(_db: &dyn Db, _def: Definition) -> Type {
    Type::Bottom
}

fn def_cycle_recover(
    _db: &dyn Db,
    value: &Type,
    count: u32,
    _def: Definition,
) -> CycleRecoveryAction<Type> {
    cycle_recover(value, count)
}

fn use_cycle_initial(_db: &dyn Db, _use: Use) -> Type {
    Type::Bottom
}

fn use_cycle_recover(
    _db: &dyn Db,
    value: &Type,
    count: u32,
    _use: Use,
) -> CycleRecoveryAction<Type> {
    cycle_recover(value, count)
}

fn cycle_recover(value: &Type, count: u32) -> CycleRecoveryAction<Type> {
    match value {
        Type::Bottom => CycleRecoveryAction::Iterate,
        Type::Values(_) => {
            if count > 4 {
                CycleRecoveryAction::Fallback(Type::Top)
            } else {
                CycleRecoveryAction::Iterate
            }
        }
        Type::Top => CycleRecoveryAction::Iterate,
    }
}

fn add(a: &Type, b: &Type) -> Type {
    match (a, b) {
        (Type::Bottom, _) | (_, Type::Bottom) => Type::Bottom,
        (Type::Top, _) | (_, Type::Top) => Type::Top,
        (Type::Values(a_ints), Type::Values(b_ints)) => {
            let mut set = BTreeSet::new();
            set.extend(
                a_ints
                    .into_iter()
                    .flat_map(|a| b_ints.into_iter().map(move |b| a + b)),
            );
            Type::Values(set.into_iter().collect())
        }
    }
}

fn dataflow(criterion: &mut Criterion) {
    criterion.bench_function("converge_diverge", |b| {
        b.iter_batched_ref(
            || {
                let mut db = salsa::DatabaseImpl::new();

                let defx0 = Definition::new(&db, None, 0);
                let defy0 = Definition::new(&db, None, 0);
                let defx1 = Definition::new(&db, None, 0);
                let defy1 = Definition::new(&db, None, 0);
                let use_x = Use::new(&db, vec![defx0, defx1]);
                let use_y = Use::new(&db, vec![defy0, defy1]);
                defx1.set_base(&mut db).to(Some(use_y));
                defy1.set_base(&mut db).to(Some(use_x));

                // prewarm cache
                let _ = infer_use(&db, use_x);
                let _ = infer_use(&db, use_y);

                (db, defx1, use_x, use_y)
            },
            |(db, defx1, use_x, use_y)| {
                // Set the increment on x to 0.
                defx1.set_increment(db).to(0);

                // Both symbols converge on 0.
                assert_eq!(infer_use(db, *use_x), Type::Values(Box::from([0])));
                assert_eq!(infer_use(db, *use_y), Type::Values(Box::from([0])));

                // Set the increment on x to 1.
                defx1.set_increment(db).to(1);

                // Now the loop diverges and we fall back to Top.
                assert_eq!(infer_use(db, *use_x), Type::Top);
                assert_eq!(infer_use(db, *use_y), Type::Top);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, dataflow);
criterion_main!(benches);
