/// Minimal example use case for fixpoint iteration cycle resolution.
use salsa::{Database as Db, Setter};

/// A Use of a symbol.
#[salsa::input]
struct Use {
    reaching_definitions: Vec<Definition>,
}

#[salsa::input]
struct Literal {
    value: LiteralValue,
}

#[derive(Clone, Debug)]
enum LiteralValue {
    Int(usize),
    Str(String),
}

/// A Definition of a symbol, either of the form `base + increment` or `0 + increment`.
#[salsa::input]
struct Definition {
    base: Option<Use>,
    increment: Literal,
}

#[derive(Eq, PartialEq, Clone, Debug)]
enum Type {
    Unbound,
    LiteralInt(usize),
    LiteralStr(String),
    Int,
    Str,
    Union(Vec<Type>),
}

#[salsa::tracked]
fn infer_use<'db>(db: &'db dyn Db, u: Use) -> Type {
    let defs = u.reaching_definitions(db);
    match defs[..] {
        [] => Type::Unbound,
        [def] => infer_definition(db, def),
        _ => Type::Union(defs.iter().map(|&def| infer_definition(db, def)).collect()),
    }
}

#[salsa::tracked]
fn infer_definition<'db>(db: &'db dyn Db, def: Definition) -> Type {
    let increment_ty = infer_literal(db, def.increment(db));
    if let Some(base) = def.base(db) {
        let base_ty = infer_use(db, base);
        match (base_ty, increment_ty) {
            (Type::Unbound, _) => panic!("unbound use"),
            (Type::LiteralInt(b), Type::LiteralInt(i)) => Type::LiteralInt(b + i),
            (Type::LiteralStr(b), Type::LiteralStr(i)) => Type::LiteralStr(format!("{}{}", b, i)),
            (Type::Int, Type::LiteralInt(_)) => Type::Int,
            (Type::LiteralInt(_), Type::Int) => Type::Int,
            (Type::Str, Type::LiteralStr(_)) => Type::Str,
            (Type::LiteralStr(_), Type::Str) => Type::Str,
            _ => panic!("type error"),
        }
    } else {
        increment_ty
    }
}

#[salsa::tracked]
fn infer_literal<'db>(db: &'db dyn Db, literal: Literal) -> Type {
    match literal.value(db) {
        LiteralValue::Int(i) => Type::LiteralInt(i),
        LiteralValue::Str(s) => Type::LiteralStr(s),
    }
}

/// x = 1
#[test]
fn simple() {
    let db = salsa::DatabaseImpl::new();

    let def = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(1)));
    let u = Use::new(&db, vec![def]);

    let ty = infer_use(&db, u);

    assert_eq!(ty, Type::LiteralInt(1));
}

/// x = "a" if flag else "b"
#[test]
fn union() {
    let db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(
        &db,
        None,
        Literal::new(&db, LiteralValue::Str("a".to_string())),
    );
    let def2 = Definition::new(
        &db,
        None,
        Literal::new(&db, LiteralValue::Str("b".to_string())),
    );
    let u = Use::new(&db, vec![def1, def2]);

    let ty = infer_use(&db, u);

    assert_eq!(
        ty,
        Type::Union(vec![
            Type::LiteralStr("a".to_string()),
            Type::LiteralStr("b".to_string())
        ])
    );
}

/// x = 1; loop { x = x + 0 }
#[test]
fn cycle_converges() {
    let mut db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(1)));
    let def2 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(0)));
    let u = Use::new(&db, vec![def1, def2]);
    def2.set_base(&mut db).to(Some(u));

    let ty = infer_use(&db, u);

    // Loop converges on LiteralInt(1)
    assert_eq!(ty, Type::LiteralInt(1));
}

/// x = 1; loop { x = x + 1 }
#[test]
fn cycle_diverges() {
    let mut db = salsa::DatabaseImpl::new();

    let def1 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(1)));
    let def2 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(1)));
    let u = Use::new(&db, vec![def1, def2]);
    def2.set_base(&mut db).to(Some(u));

    let ty = infer_use(&db, u);

    // Loop diverges. Cut it off and fallback from "all LiteralInt observed" to Type::Int
    assert_eq!(ty, Type::Int);
}

/// x = 0; y = 0; loop { x = y + 0; y = x + 0 }
#[test]
fn multi_symbol_cycle_converges() {
    let mut db = salsa::DatabaseImpl::new();

    let defx0 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(0)));
    let defy0 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(0)));
    let defx1 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(0)));
    let defy1 = Definition::new(&db, None, Literal::new(&db, LiteralValue::Int(0)));
    let use_x = Use::new(&db, vec![defx0, defx1]);
    let use_y = Use::new(&db, vec![defy0, defy1]);
    defx1.set_base(&mut db).to(Some(use_y));
    defy1.set_base(&mut db).to(Some(use_x));

    let x_ty = infer_use(&db, use_x);
    let y_ty = infer_use(&db, use_y);

    // Both symbols converge on LiteralInt(0)
    assert_eq!(x_ty, Type::LiteralInt(0));
    assert_eq!(y_ty, Type::LiteralInt(0));
}
