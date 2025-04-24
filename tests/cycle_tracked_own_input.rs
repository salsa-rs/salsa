//! Tests for cycles where the cycle head is stored on a tracked struct
//! and that tracked struct is freed in a later revision.

mod common;

use crate::common::{EventLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{CycleRecoveryAction, Database, Setter};

#[salsa::input(debug)]
struct ClassNode {
    name: String,
    type_params: Option<TypeParamNode>,
}

#[salsa::input(debug)]
struct TypeParamNode {
    name: String,
    constraint: Option<ClassNode>,
}

#[salsa::interned(debug)]
struct Class<'db> {
    name: String,
    type_params: Option<TypeParam<'db>>,
}

#[salsa::tracked(debug)]
struct TypeParam<'db> {
    name: String,
    constraint: Option<Type<'db>>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, salsa::Update)]
enum Type<'db> {
    Class(Class<'db>),
    Unknown,
}

#[salsa::tracked(cycle_fn=infer_class_recover, cycle_initial=infer_class_initial)]
fn infer_class<'db>(db: &'db dyn salsa::Database, node: ClassNode) -> Type<'db> {
    Type::Class(Class::new(
        db,
        node.name(db),
        node.type_params(db).map(|tp| infer_type_param(db, tp)),
    ))
}

#[salsa::tracked]
fn infer_type_param<'db>(db: &'db dyn salsa::Database, node: TypeParamNode) -> TypeParam<'db> {
    // What's missing here is that we need to read the type param from the previous iteration in this function!
    if let Some(constraint) = node.constraint(db) {
        match infer_class(db, constraint) {
            Type::Class(class) => {
                if let Some(type_param) = class.type_params(db) {
                    print!(
                        "Return type param from last iteration with name: {}",
                        type_param.name(db)
                    );
                    type_param
                } else {
                    TypeParam::new(db, node.name(db), Some(Type::Unknown))
                }
            }
            Type::Unknown => TypeParam::new(db, node.name(db), Some(Type::Unknown)),
        }
    } else {
        TypeParam::new(db, node.name(db), None)
    }
}

fn infer_class_initial(_db: &dyn Database, _node: ClassNode) -> Type {
    Type::Unknown
}

fn infer_class_recover<'db>(
    _db: &'db dyn Database,
    _type: &Type<'db>,
    _count: u32,
    _inputs: ClassNode,
) -> CycleRecoveryAction<Type<'db>> {
    CycleRecoveryAction::Iterate
}

// #[test]
#[test_log::test]
fn main() {
    let mut db = EventLoggerDatabase::default();

    let class_node = ClassNode::new(&db, "Test".to_string(), None);
    let type_param_node = TypeParamNode::new(&db, "T".to_string(), Some(class_node));
    class_node
        .set_type_params(&mut db)
        .to(Some(type_param_node));

    let ty = infer_class(&db, class_node);

    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: infer_class(Id(0)) }",
            "WillCheckCancellation",
            "WillExecute { database_key: infer_type_param(Id(400)) }",
            "WillCheckCancellation",
            "DidInternValue { key: Class(Id(c00)), revision: R2 }",
            "WillIterateCycle { database_key: infer_class(Id(0)), iteration_count: 1, fell_back: false }",
            "WillCheckCancellation",
            "WillExecute { database_key: infer_type_param(Id(400)) }",
            "WillCheckCancellation",
            "WillDiscardStaleOutput { execute_key: infer_type_param(Id(400)), output_key: TypeParam(Id(800)) }",
            "DidDiscard { key: TypeParam(Id(800)) }",
        ]"#]]);

    if let Type::Class(class) = ty {
        if let Some(type_param) = class.type_params(&db) {
            // This panics because it points to the now discarded TypeParam(Id(800))
            // It got discarded because Salsa no longer recognizes that the tracked struct was created by infer_type_param(Id(400)) because it
            // read the type_var from `class`.
            assert_eq!(type_param.name(&db), "T");
        }
    }
}
