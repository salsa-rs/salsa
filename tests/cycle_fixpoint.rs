/// Test case for fixpoint iteration cycle resolution.
use salsa::{CycleRecoveryAction, Database as Db, Setter};
use std::collections::BTreeSet;
use std::iter::IntoIterator;

/// A Use of a symbol.
#[salsa::input]
struct Use {
    reaching_definitions: Vec<Definition>,
}

#[salsa::input]
struct Literal {
    value: usize,
}

/// A Definition of a symbol, either of the form `base + increment` or `0 + increment`.
#[salsa::input]
struct Definition {
    base: Option<Use>,
    increment: Literal,
}

#[derive(Eq, PartialEq, Clone, Debug)]
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

#[salsa::tracked(cycle_fn=cycle_recover, cycle_initial=cycle_initial)]
fn infer_use<'db>(db: &'db dyn Db, u: Use) -> Type {
    let defs = u.reaching_definitions(db);
    match defs[..] {
        [] => Type::Bottom,
        [def] => infer_definition(db, def),
        _ => Type::join(defs.iter().map(|&def| infer_definition(db, def))),
    }
}

#[salsa::tracked(cycle_fn=cycle_recover, cycle_initial=cycle_initial)]
fn infer_definition<'db>(db: &'db dyn Db, def: Definition) -> Type {
    let increment_ty = infer_literal(db, def.increment(db));
    if let Some(base) = def.base(db) {
        let base_ty = infer_use(db, base);
        add(&base_ty, &increment_ty)
    } else {
        increment_ty
    }
}

fn cycle_initial<'db>(_db: &'db dyn Db) -> Type {
    Type::Bottom
}

fn cycle_recover<'db>(_db: &'db dyn Db, value: Type, count: u32) -> CycleRecoveryAction<Type> {
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

#[salsa::tracked]
fn infer_literal<'db>(db: &'db dyn Db, literal: Literal) -> Type {
    Type::Values(Box::from([literal.value(db)]))
}

/// x = 1
#[test]
fn simple() {
    let db = salsa::DatabaseImpl::new();

    let def = Definition::new(&db, None, Literal::new(&db, 1));
    let u = Use::new(&db, vec![def]);

    let ty = infer_use(&db, u);

    assert_eq!(ty, Type::Values(Box::from([1])));
}

/// x = 1 if flag else 2
#[test]
fn union() {
    let db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(&db, None, Literal::new(&db, 1));
    let def2 = Definition::new(&db, None, Literal::new(&db, 2));
    let u = Use::new(&db, vec![def1, def2]);

    let ty = infer_use(&db, u);

    assert_eq!(ty, Type::Values(Box::from([1, 2])));
}

/// x = 1 if flag else 2; y = x + 1
#[test]
fn union_add() {
    let db = salsa::DatabaseImpl::new();

    let x1 = Definition::new(&db, None, Literal::new(&db, 1));
    let x2 = Definition::new(&db, None, Literal::new(&db, 2));
    let x_use = Use::new(&db, vec![x1, x2]);
    let y_def = Definition::new(&db, Some(x_use), Literal::new(&db, 1));
    let y_use = Use::new(&db, vec![y_def]);

    let ty = infer_use(&db, y_use);

    assert_eq!(ty, Type::Values(Box::from([2, 3])));
}

/// x = 1; loop { x = x + 0 }
#[test]
fn cycle_converges() {
    let mut db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(&db, None, Literal::new(&db, 1));
    let def2 = Definition::new(&db, None, Literal::new(&db, 0));
    let u = Use::new(&db, vec![def1, def2]);
    def2.set_base(&mut db).to(Some(u));

    let ty = infer_use(&db, u);

    // Loop converges on 1
    assert_eq!(ty, Type::Values(Box::from([1])));
}

/// x = 1; loop { x = x + 1 }
#[test]
fn cycle_diverges() {
    let mut db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(&db, None, Literal::new(&db, 1));
    let def2 = Definition::new(&db, None, Literal::new(&db, 1));
    let u = Use::new(&db, vec![def1, def2]);
    def2.set_base(&mut db).to(Some(u));

    let ty = infer_use(&db, u);

    // Loop diverges. Cut it off and fallback to Type::Top
    assert_eq!(ty, Type::Top);
}

/// x = 0; y = 0; loop { x = y + 0; y = x + 0 }
#[test]
fn multi_symbol_cycle_converges() {
    let mut db = salsa::DatabaseImpl::new();

    let defx0 = Definition::new(&db, None, Literal::new(&db, 0));
    let defy0 = Definition::new(&db, None, Literal::new(&db, 0));
    let defx1 = Definition::new(&db, None, Literal::new(&db, 0));
    let defy1 = Definition::new(&db, None, Literal::new(&db, 0));
    let use_x = Use::new(&db, vec![defx0, defx1]);
    let use_y = Use::new(&db, vec![defy0, defy1]);
    defx1.set_base(&mut db).to(Some(use_y));
    defy1.set_base(&mut db).to(Some(use_x));

    let x_ty = infer_use(&db, use_x);
    let y_ty = infer_use(&db, use_y);

    // Both symbols converge on 0
    assert_eq!(x_ty, Type::Values(Box::from([0])));
    assert_eq!(y_ty, Type::Values(Box::from([0])));
}
