#![cfg(feature = "inventory")]

use expect_test::expect;

#[salsa::input(heap_size = string_tuple_size_of)]
struct MyInput {
    field: String,
}

#[salsa::tracked(heap_size = string_tuple_size_of)]
struct MyTracked<'db> {
    field: String,
}

#[salsa::interned(heap_size = string_tuple_size_of)]
struct MyInterned<'db> {
    field: String,
}

#[salsa::tracked]
fn input_to_interned<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyInterned<'db> {
    MyInterned::new(db, input.field(db))
}

#[salsa::tracked]
fn input_to_tracked<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyTracked<'db> {
    MyTracked::new(db, input.field(db))
}

#[salsa::tracked]
fn input_to_string<'db>(_db: &'db dyn salsa::Database) -> String {
    "a".repeat(1000)
}

#[salsa::tracked(heap_size = string_size_of)]
fn input_to_string_get_size<'db>(_db: &'db dyn salsa::Database) -> String {
    "a".repeat(1000)
}

fn string_size_of(x: &String) -> usize {
    x.capacity()
}

fn string_tuple_size_of((x,): &(String,)) -> usize {
    x.capacity()
}

#[salsa::tracked]
fn input_to_tracked_tuple<'db>(
    db: &'db dyn salsa::Database,
    input: MyInput,
) -> (MyTracked<'db>, MyTracked<'db>) {
    (
        MyTracked::new(db, input.field(db)),
        MyTracked::new(db, input.field(db)),
    )
}

#[test]
fn test() {
    let db = salsa::DatabaseImpl::new();

    let input1 = MyInput::new(&db, "a".repeat(50));
    let input2 = MyInput::new(&db, "a".repeat(150));
    let input3 = MyInput::new(&db, "a".repeat(250));

    let _tracked1 = input_to_tracked(&db, input1);
    let _tracked2 = input_to_tracked(&db, input2);

    let _tracked_tuple = input_to_tracked_tuple(&db, input1);

    let _interned1 = input_to_interned(&db, input1);
    let _interned2 = input_to_interned(&db, input2);
    let _interned3 = input_to_interned(&db, input3);

    let _string1 = input_to_string(&db);
    let _string2 = input_to_string_get_size(&db);

    let structs_info = <dyn salsa::Database>::structs_info(&db, salsa::PanicIfHeapSizeMissing::No);

    let expected = expect![[r#"
        [
            IngredientInfo {
                debug_name: "MyInput",
                count: 3,
                size_of_metadata: 96,
                size_of_fields: 522,
            },
            IngredientInfo {
                debug_name: "MyTracked",
                count: 4,
                size_of_metadata: 128,
                size_of_fields: 396,
            },
            IngredientInfo {
                debug_name: "MyInterned",
                count: 3,
                size_of_metadata: 168,
                size_of_fields: 522,
            },
            IngredientInfo {
                debug_name: "input_to_string::interned_arguments",
                count: 1,
                size_of_metadata: 56,
                size_of_fields: 0,
            },
            IngredientInfo {
                debug_name: "input_to_string_get_size::interned_arguments",
                count: 1,
                size_of_metadata: 56,
                size_of_fields: 0,
            },
        ]"#]];

    expected.assert_eq(&format!("{structs_info:#?}"));

    let mut queries_info =
        <dyn salsa::Database>::queries_info(&db, salsa::PanicIfHeapSizeMissing::No)
            .into_iter()
            .collect::<Vec<_>>();
    queries_info.sort();

    let expected = expect![[r#"
        [
            (
                "input_to_interned",
                IngredientInfo {
                    debug_name: "memory_usage::MyInterned",
                    count: 3,
                    size_of_metadata: 192,
                    size_of_fields: 24,
                },
            ),
            (
                "input_to_string",
                IngredientInfo {
                    debug_name: "alloc::string::String",
                    count: 1,
                    size_of_metadata: 40,
                    size_of_fields: 24,
                },
            ),
            (
                "input_to_string_get_size",
                IngredientInfo {
                    debug_name: "alloc::string::String",
                    count: 1,
                    size_of_metadata: 40,
                    size_of_fields: 1024,
                },
            ),
            (
                "input_to_tracked",
                IngredientInfo {
                    debug_name: "memory_usage::MyTracked",
                    count: 2,
                    size_of_metadata: 192,
                    size_of_fields: 16,
                },
            ),
            (
                "input_to_tracked_tuple",
                IngredientInfo {
                    debug_name: "(memory_usage::MyTracked, memory_usage::MyTracked)",
                    count: 1,
                    size_of_metadata: 132,
                    size_of_fields: 16,
                },
            ),
        ]"#]];

    expected.assert_eq(&format!("{queries_info:#?}"));
}

#[test]
#[should_panic = "tried to estimate sizes but `heap_size()` was not defined.\ningredient: MyInput"]
fn missing_coverage() {
    #[salsa::input]
    struct MyInput {}

    let db = salsa::DatabaseImpl::default();
    let _input = MyInput::new(&db);

    <dyn salsa::Database>::structs_info(&db, salsa::PanicIfHeapSizeMissing::Yes);
}
