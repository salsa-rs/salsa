#![cfg(feature = "inventory")]

use std::collections::BTreeMap;

use expect_test::expect;

use salsa::{MemoMemoryInfo, MemoryUsageVisitor, StructMemoryInfo};

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

fn string_size_of(x: &String, visitor: &mut dyn MemoryUsageVisitor) -> usize {
    visitor.add_detail("String", x.capacity());

    x.capacity()
}

fn string_tuple_size_of((x,): &(String,), visitor: &mut dyn MemoryUsageVisitor) -> usize {
    if let Some(visitor) = (visitor as &mut dyn std::any::Any).downcast_mut::<Visitor>() {
        visitor.visit_tuple(2);
    }

    visitor.add_detail("String", x.capacity());

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

#[derive(Debug)]
struct IngredientInfo {
    count: usize,
    size_of_metadata: usize,
    size_of_fields: usize,
    heap_size_of_fields: usize,
    #[allow(unused)]
    kind: SlotKind,
}

struct MemoryInfo {
    size_of_metadata: usize,
    size_of_fields: usize,
    heap_size_of_fields: usize,
}

impl From<StructMemoryInfo> for MemoryInfo {
    fn from(info: StructMemoryInfo) -> Self {
        MemoryInfo {
            size_of_metadata: info.size_of_metadata(),
            size_of_fields: info.size_of_fields(),
            heap_size_of_fields: info.heap_size_of_fields().unwrap_or_default(),
        }
    }
}

impl From<MemoMemoryInfo> for MemoryInfo {
    fn from(value: MemoMemoryInfo) -> Self {
        MemoryInfo {
            size_of_metadata: value.size_of_metadata(),
            size_of_fields: value.size_of_fields(),
            heap_size_of_fields: value.heap_size_of_fields().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum SlotKind {
    Input,
    Tracked,
    Interned,
    Memo,
}

#[derive(Default, Debug)]
struct Visitor {
    slots: BTreeMap<&'static str, IngredientInfo>,
    custom: BTreeMap<&'static str, usize>,
    max_tuple_elements: usize,
}

impl Visitor {
    fn add(&mut self, name: &'static str, info: MemoryInfo, kind: SlotKind) {
        let aggregated = self.slots.entry(name).or_insert_with(|| IngredientInfo {
            count: 0,
            size_of_metadata: 0,
            size_of_fields: 0,
            heap_size_of_fields: 0,
            kind,
        });

        aggregated.count += 1;
        aggregated.size_of_metadata += info.size_of_metadata;
        aggregated.size_of_fields += info.size_of_fields;
        aggregated.heap_size_of_fields += info.heap_size_of_fields;
    }

    fn visit_tuple(&mut self, elements: usize) {
        self.max_tuple_elements = self.max_tuple_elements.max(elements);
    }
}

impl MemoryUsageVisitor for Visitor {
    fn visit_input_struct(&mut self, slot: salsa::StructMemoryInfo) {
        self.add(slot.debug_name(), slot.into(), SlotKind::Input);
    }

    fn visit_interned_struct(&mut self, slot: salsa::StructMemoryInfo) {
        self.add(slot.debug_name(), slot.into(), SlotKind::Interned);
    }

    fn visit_tracked_struct(&mut self, slot: salsa::StructMemoryInfo) {
        self.add(slot.debug_name(), slot.into(), SlotKind::Tracked);
    }

    fn visit_memo(&mut self, slot: salsa::MemoMemoryInfo) {
        self.add(slot.query_debug_name(), slot.into(), SlotKind::Memo);
    }

    fn add_detail(&mut self, name: &'static str, size: usize) {
        *self.custom.entry(name).or_default() += size;
    }
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

    let mut visitor = Visitor::default();
    <dyn salsa::Database>::memory_usage(&db, &mut visitor);

    let expected = expect![[r#"
        Visitor {
            slots: {
                "MyInput": IngredientInfo {
                    count: 3,
                    size_of_metadata: 96,
                    size_of_fields: 72,
                    heap_size_of_fields: 450,
                    kind: Input,
                },
                "input_to_interned": IngredientInfo {
                    count: 3,
                    size_of_metadata: 192,
                    size_of_fields: 24,
                    heap_size_of_fields: 0,
                    kind: Memo,
                },
                "input_to_string": IngredientInfo {
                    count: 1,
                    size_of_metadata: 40,
                    size_of_fields: 24,
                    heap_size_of_fields: 0,
                    kind: Memo,
                },
                "input_to_string_get_size": IngredientInfo {
                    count: 1,
                    size_of_metadata: 40,
                    size_of_fields: 24,
                    heap_size_of_fields: 1000,
                    kind: Memo,
                },
                "input_to_tracked": IngredientInfo {
                    count: 2,
                    size_of_metadata: 192,
                    size_of_fields: 16,
                    heap_size_of_fields: 0,
                    kind: Memo,
                },
                "input_to_tracked_tuple": IngredientInfo {
                    count: 1,
                    size_of_metadata: 132,
                    size_of_fields: 16,
                    heap_size_of_fields: 0,
                    kind: Memo,
                },
            },
            custom: {
                "String": 2200,
            },
            max_tuple_elements: 2,
        }"#]];

    expected.assert_eq(&format!("{visitor:#?}"));
}
