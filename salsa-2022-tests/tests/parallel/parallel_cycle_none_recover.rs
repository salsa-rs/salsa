use crate::setup::Knobs;
use crate::setup::Database as DatabaseImpl;
use expect_test::expect;
use salsa::ParallelDatabase;
use crate::setup::Jar;
use crate::setup::Db;

#[salsa::input(jar = Jar)]
pub(crate) struct MyInput {
    field: i32
}

#[salsa::tracked(jar = Jar)]
pub(crate) fn a(db: &dyn Db, input: MyInput) -> i32 {
    db.signal(1);
    db.wait_for(2);

    b(db, input)
}

#[salsa::tracked(jar = Jar)]
pub(crate) fn b(db: &dyn Db, input: MyInput) -> i32 {
    db.wait_for(1);
    db.signal(2);

    db.wait_for(3);
    a(db, input)
}

#[test]
fn execute() {
    let mut db = DatabaseImpl::default();
    db.knobs().signal_on_will_block.set(3);

    let input = MyInput::new(&mut db, -1);

    std::thread::spawn({
        let db = db.snapshot();
        move || a(&*db, input)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || b(&*db, input)
    });
    let err_b = thread_b.join().unwrap_err();
    if let Some(c) = err_b.downcast_ref::<salsa::Cycle>() {
        let expected = expect![[r#"
            [
                "DependencyIndex { ingredient_index: IngredientIndex(2), key_index: Some(Id { value: 1 }) }",
                "DependencyIndex { ingredient_index: IngredientIndex(3), key_index: Some(Id { value: 1 }) }",
            ]
        "#]];
        expected.assert_debug_eq(&c.all_participants(&db));
    } else {
        panic!("b failed in an unexpected way: {:?}", err_b);
    }
}