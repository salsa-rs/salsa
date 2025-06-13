#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: u32,
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

    let input1 = MyInput::new(&db, 1);
    let input2 = MyInput::new(&db, 2);
    let input3 = MyInput::new(&db, 3);

    let _tracked1 = input_to_tracked(&db, input1);
    let _tracked2 = input_to_tracked(&db, input2);

    let _tracked_tuple = input_to_tracked_tuple(&db, input1);

    let _interned1 = input_to_interned(&db, input1);
    let _interned2 = input_to_interned(&db, input2);
    let _interned3 = input_to_interned(&db, input3);

    let structs_info = <dyn salsa::Database>::structs_info(&db);

    assert_eq!(
        format!("{structs_info:#?}"),
        r#"[
    IngredientInfo {
        debug_name: "MyInput",
        count: 3,
        size_of_metadata: 84,
        size_of_fields: 12,
    },
    IngredientInfo {
        debug_name: "MyTracked",
        count: 4,
        size_of_metadata: 112,
        size_of_fields: 16,
    },
    IngredientInfo {
        debug_name: "MyInterned",
        count: 3,
        size_of_metadata: 156,
        size_of_fields: 12,
    },
]"#
    );

    let mut queries_info = <dyn salsa::Database>::queries_info(&db)
        .into_iter()
        .collect::<Vec<_>>();
    queries_info.sort();

    assert_eq!(
        format!("{queries_info:#?}"),
        r#"[
    (
        (
            "MyInput",
            "(memory_usage::MyTracked, memory_usage::MyTracked)",
        ),
        IngredientInfo {
            debug_name: "(memory_usage::MyTracked, memory_usage::MyTracked)",
            count: 1,
            size_of_metadata: 132,
            size_of_fields: 16,
        },
    ),
    (
        (
            "MyInput",
            "memory_usage::MyInterned",
        ),
        IngredientInfo {
            debug_name: "memory_usage::MyInterned",
            count: 3,
            size_of_metadata: 192,
            size_of_fields: 24,
        },
    ),
    (
        (
            "MyInput",
            "memory_usage::MyTracked",
        ),
        IngredientInfo {
            debug_name: "memory_usage::MyTracked",
            count: 2,
            size_of_metadata: 192,
            size_of_fields: 16,
        },
    ),
]"#
    )
}
