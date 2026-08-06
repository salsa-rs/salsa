#![cfg(feature = "inventory")]

use salsa::Database;
use test_log::test;

#[salsa::interned]
struct InternedString<'db> {
    data: String,
    #[self_ref]
    other: InternedString<'db>,
}

#[salsa::interned]
struct SelfOnly<'db> {
    #[self_ref]
    other: SelfOnly<'db>,
}

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct SelfOnlyNoLifetime {
    #[self_ref]
    other: SelfOnlyNoLifetime,
}

#[salsa::interned]
struct Interleaved<'db> {
    first: String,
    #[self_ref]
    other: Interleaved<'db>,
    second: u32,
}

#[salsa::interned]
struct MultipleSelfRefs<'db> {
    key: u32,
    #[self_ref]
    first: MultipleSelfRefs<'db>,
    #[self_ref]
    second: MultipleSelfRefs<'db>,
}

#[salsa::interned(debug)]
struct DebugRecursive<'db> {
    key: u32,
    #[self_ref]
    other: DebugRecursive<'db>,
}

#[salsa::interned(heap_size = self_ref_heap_size)]
struct HeapRecursive<'db> {
    data: String,
    #[self_ref]
    other: HeapRecursive<'db>,
}

fn self_ref_heap_size((data, _other): &(String, HeapRecursive<'_>)) -> usize {
    data.capacity()
}

#[test]
fn self_ref_fields_accept_explicit_or_self_values() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, ".to_string(), None);
    let s2 = InternedString::new(&db, "World, ".to_string(), Some(s1));

    assert!(*s1.other(&db) == s1);
    assert!(*s2.other(&db) == s1);

    let s1_again = InternedString::new(&db, "Hello, ", Some(s2));
    let s2_again = InternedString::new(&db, "World, ", None);

    assert!(s1_again != s1);
    assert!(s2_again != s2);
    assert!(*s1_again.other(&db) == s2);
    assert!(*s2_again.other(&db) == s2_again);
}

#[test]
fn self_ref_can_be_the_only_field() {
    let db = salsa::DatabaseImpl::new();
    let value = SelfOnly::new(&db, None);

    assert!(*value.other(&db) == value);
    assert!(SelfOnly::new(&db, Some(value)) == value);
}

#[test]
fn self_ref_supports_no_lifetime() {
    let db = salsa::DatabaseImpl::new();
    let value = SelfOnlyNoLifetime::new(&db, None);

    assert!(*value.other(&db) == value);
}

#[test]
fn self_ref_can_be_interleaved_with_identity_fields() {
    let db = salsa::DatabaseImpl::new();
    let value = Interleaved::new(&db, "first".to_string(), None, 1);
    let other = Interleaved::new(&db, "other".to_string(), None, 2);

    let explicit = Interleaved::new(&db, "first", Some(other), 1);

    assert!(explicit != value);
    assert!(Interleaved::new(&db, "different", None, 1) != value);
    assert!(Interleaved::new(&db, "first", None, 2) != value);
    assert!(value.first(&db) == "first");
    assert!(*value.other(&db) == value);
    assert!(*value.second(&db) == 1);
    assert!(*explicit.other(&db) == other);
}

#[test]
fn multiple_self_ref_fields_are_assembled_independently() {
    let db = salsa::DatabaseImpl::new();
    let anchor = MultipleSelfRefs::new(&db, 0, None, None);
    let both_self = MultipleSelfRefs::new(&db, 1, None, None);
    let first_self = MultipleSelfRefs::new(&db, 2, None, Some(anchor));
    let second_self = MultipleSelfRefs::new(&db, 3, Some(anchor), None);

    assert!(*both_self.first(&db) == both_self);
    assert!(*both_self.second(&db) == both_self);
    assert!(*first_self.first(&db) == first_self);
    assert!(*first_self.second(&db) == anchor);
    assert!(*second_self.first(&db) == anchor);
    assert!(*second_self.second(&db) == second_self);
}

#[test]
fn debug_formats_self_ref_fields_by_id() {
    use salsa::plumbing::AsId;

    salsa::DatabaseImpl::new().attach(|db| {
        let value = DebugRecursive::new(db, 0, None);
        let value_id = value.as_id();
        let other = DebugRecursive::new(db, 1, Some(value));

        assert_eq!(
            format!("{value:?}"),
            format!("DebugRecursive {{ key: 0, other: {value_id:?} }}")
        );
        assert_eq!(
            format!("{other:?}"),
            format!(
                "DebugRecursive {{ key: 1, other: DebugRecursive {{ key: 0, other: {value_id:?} }} }}"
            )
        );
    });
}

#[test]
fn heap_size_uses_stored_fields() {
    let db = salsa::DatabaseImpl::new();
    let mut value_data = String::with_capacity(32);
    value_data.push_str("one");
    let value_capacity = value_data.capacity();
    let value = HeapRecursive::new(&db, value_data, None);

    let mut other_data = String::with_capacity(64);
    other_data.push_str("four");
    let other_capacity = other_data.capacity();
    let _other = HeapRecursive::new(&db, other_data, Some(value));

    let memory_usage = <dyn salsa::Database>::memory_usage(&db);
    let ingredient = memory_usage
        .structs
        .iter()
        .find(|ingredient| ingredient.debug_name() == "HeapRecursive")
        .unwrap();

    assert_eq!(
        ingredient.heap_size_of_fields(),
        Some(value_capacity + other_capacity)
    );
}
