#![cfg(feature = "inventory")]

mod common;

use common::{DiscardLoggerDatabase, LogDatabase, LoggerDatabase};
use expect_test::expect;
use salsa::Setter;
use test_log::test;

#[salsa::input(debug)]
struct Number {
    value: u32,
}

#[salsa::input(debug)]
struct Switch {
    value: u32,
    use_previous: bool,
}

#[salsa::tracked]
struct PreviousNode<'db> {
    value: u32,
}

#[salsa::tracked]
struct NamedNode<'db> {
    name: String,
    #[tracked]
    value: u32,
}

#[salsa::tracked]
fn previous_plus_one(db: &dyn LogDatabase, input: Number) -> u32 {
    db.push_log("previous_plus_one".to_owned());

    let Some(previous) = previous_plus_one::previous(db) else {
        return input.value(db);
    };

    previous + 1
}

#[salsa::tracked]
fn read_previous_or_value(db: &dyn LogDatabase, input: Switch) -> u32 {
    db.push_log("read_previous_or_value".to_owned());

    if input.use_previous(db) {
        let previous = read_previous_or_value::previous(db).unwrap();
        return *previous;
    }

    input.value(db)
}

#[salsa::tracked]
fn depends_on_previous_plus_one(db: &dyn LogDatabase, input: Number) -> u32 {
    db.push_log("depends_on_previous_plus_one".to_owned());
    previous_plus_one(db, input)
}

#[salsa::tracked]
fn previous_node(db: &dyn LogDatabase, input: Switch) -> PreviousNode<'_> {
    db.push_log("previous_node".to_owned());

    if input.use_previous(db) {
        let previous = previous_node::previous(db).unwrap();
        return *previous;
    }

    PreviousNode::new(db, input.value(db))
}

#[salsa::tracked]
fn previous_then_recreate_same_identity(
    db: &dyn LogDatabase,
    input: Switch,
) -> (NamedNode<'_>, NamedNode<'_>) {
    db.push_log("previous_then_recreate_same_identity".to_owned());

    if input.use_previous(db) {
        let previous = previous_then_recreate_same_identity::previous(db).unwrap();
        let previous = previous.0;
        previous.value(db);
        let current = NamedNode::new(db, "node".to_owned(), input.value(db));
        return (previous, current);
    }

    let node = NamedNode::new(db, "node".to_owned(), input.value(db));
    (node, node)
}

#[salsa::tracked]
fn recreate_same_identity_then_previous(
    db: &dyn LogDatabase,
    input: Switch,
) -> (NamedNode<'_>, NamedNode<'_>) {
    db.push_log("recreate_same_identity_then_previous".to_owned());

    if input.use_previous(db) {
        let current = NamedNode::new(db, "node".to_owned(), input.value(db));
        let previous = recreate_same_identity_then_previous::previous(db).unwrap();
        let previous = previous.0;
        return (current, previous);
    }

    let node = NamedNode::new(db, "node".to_owned(), input.value(db));
    (node, node)
}

#[salsa::tracked]
fn duplicate_nodes_after_previous(db: &dyn LogDatabase, input: Switch) -> Vec<NamedNode<'_>> {
    db.push_log("duplicate_nodes_after_previous".to_owned());

    if input.use_previous(db) {
        let previous = duplicate_nodes_after_previous::previous(db).unwrap();
        let first = previous[0];
        let second = previous[1];
        let third = NamedNode::new(db, "same".to_owned(), input.value(db));
        let fourth = NamedNode::new(db, "same".to_owned(), input.value(db) + 1);
        return vec![first, second, third, fourth];
    }

    vec![
        NamedNode::new(db, "same".to_owned(), 10),
        NamedNode::new(db, "same".to_owned(), 11),
    ]
}

#[salsa::tracked]
fn mixed_order_duplicate_nodes(db: &dyn LogDatabase, input: Switch) -> Vec<NamedNode<'_>> {
    db.push_log("mixed_order_duplicate_nodes".to_owned());

    if input.use_previous(db) {
        let first = NamedNode::new(db, "same".to_owned(), input.value(db));
        let previous = mixed_order_duplicate_nodes::previous(db).unwrap();
        let second = previous[0];
        let third = previous[1];
        let fourth = NamedNode::new(db, "same".to_owned(), input.value(db) + 1);
        return vec![first, second, third, fourth];
    }

    vec![
        NamedNode::new(db, "same".to_owned(), 10),
        NamedNode::new(db, "same".to_owned(), 11),
    ]
}

#[salsa::tracked]
fn maybe_previous_node(db: &dyn LogDatabase, input: Switch) -> Option<NamedNode<'_>> {
    db.push_log("maybe_previous_node".to_owned());

    if input.use_previous(db) {
        let previous = maybe_previous_node::previous(db).unwrap();
        return *previous;
    }

    if input.value(db) == 0 {
        return None;
    }

    Some(NamedNode::new(db, "node".to_owned(), input.value(db)))
}

#[salsa::tracked]
fn source_node(db: &dyn LogDatabase, input: Number) -> NamedNode<'_> {
    db.push_log("source_node".to_owned());
    NamedNode::new(db, "source".to_owned(), input.value(db))
}

#[salsa::tracked]
fn read_source_or_previous(db: &dyn LogDatabase, switch: Switch, input: Number) -> u32 {
    db.push_log("read_source_or_previous".to_owned());

    if switch.use_previous(db) {
        let previous = read_source_or_previous::previous(db).unwrap();
        return *previous;
    }

    source_node(db, input).value(db)
}

#[test]
fn previous_value_is_available_as_a_reference() {
    let mut db = LoggerDatabase::default();
    let input = Number::new(&db, 1);

    assert_eq!(previous_plus_one(&db, input), 1);
    db.assert_logs(expect![[r#"
        [
            "previous_plus_one",
        ]"#]]);

    input.set_value(&mut db).to(2);

    assert_eq!(previous_plus_one(&db, input), 2);
    db.assert_logs(expect![[r#"
        [
            "previous_plus_one",
        ]"#]]);
}

#[test]
fn previous_value_replays_previous_dependencies() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 1, false);

    assert_eq!(read_previous_or_value(&db, input), 1);
    db.assert_logs(expect![[r#"
        [
            "read_previous_or_value",
        ]"#]]);

    input.set_use_previous(&mut db).to(true);
    assert_eq!(read_previous_or_value(&db, input), 1);
    db.assert_logs(expect![[r#"
        [
            "read_previous_or_value",
        ]"#]]);

    input.set_value(&mut db).to(2);
    assert_eq!(read_previous_or_value(&db, input), 1);
    db.assert_logs(expect![[r#"
        [
            "read_previous_or_value",
        ]"#]]);
}

#[test]
fn previous_value_marks_current_query_changed() {
    let mut db = LoggerDatabase::default();
    let input = Number::new(&db, 1);

    assert_eq!(depends_on_previous_plus_one(&db, input), 1);
    db.clear_logs();

    input.set_value(&mut db).to(2);

    assert_eq!(depends_on_previous_plus_one(&db, input), 2);
    db.assert_logs(expect![[r#"
        [
            "previous_plus_one",
            "depends_on_previous_plus_one",
        ]"#]]);
}

#[test]
fn previous_tracked_struct_output_remains_live() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 1, false);

    let first = previous_node(&db, input);
    assert_eq!(first.value(&db), 1);
    db.clear_logs();

    input.set_use_previous(&mut db).to(true);
    let second = previous_node(&db, input);

    assert_eq!(second.value(&db), 1);
    db.assert_logs(expect![[r#"
        [
            "previous_node",
        ]"#]]);
}

#[test]
fn previous_read_before_recreating_same_identity_preserves_old_tracked_field() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 1, false);

    let nodes = previous_then_recreate_same_identity(&db, input);
    assert_eq!((nodes.0.value(&db), nodes.1.value(&db)), (1, 1));
    db.clear_logs();

    input.set_value(&mut db).to(2);
    input.set_use_previous(&mut db).to(true);

    let nodes = previous_then_recreate_same_identity(&db, input);
    assert_eq!((nodes.0.value(&db), nodes.1.value(&db)), (1, 2));
    db.assert_logs(expect![[r#"
        [
            "previous_then_recreate_same_identity",
        ]"#]]);
}

#[test]
fn recreating_same_identity_before_previous_updates_previous_entity() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 1, false);

    let nodes = recreate_same_identity_then_previous(&db, input);
    assert_eq!((nodes.0.value(&db), nodes.1.value(&db)), (1, 1));
    db.clear_logs();

    input.set_value(&mut db).to(2);
    input.set_use_previous(&mut db).to(true);

    let nodes = recreate_same_identity_then_previous(&db, input);
    assert_eq!((nodes.0.value(&db), nodes.1.value(&db)), (2, 2));
    db.assert_logs(expect![[r#"
        [
            "recreate_same_identity_then_previous",
        ]"#]]);
}

#[test]
fn previous_disambiguators_prevent_duplicate_identity_collisions() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 20, false);

    let nodes = duplicate_nodes_after_previous(&db, input);
    assert_eq!(
        nodes.iter().map(|node| node.value(&db)).collect::<Vec<_>>(),
        [10, 11]
    );
    db.clear_logs();

    input.set_use_previous(&mut db).to(true);
    let nodes = duplicate_nodes_after_previous(&db, input);

    assert_eq!(
        nodes.iter().map(|node| node.value(&db)).collect::<Vec<_>>(),
        [10, 11, 20, 21]
    );
    db.assert_logs(expect![[r#"
        [
            "duplicate_nodes_after_previous",
        ]"#]]);
}

#[test]
fn duplicate_identity_before_previous_documents_live_entity_aliasing() {
    let mut db = LoggerDatabase::default();
    let input = Switch::new(&db, 20, false);

    let nodes = mixed_order_duplicate_nodes(&db, input);
    assert_eq!(
        nodes.iter().map(|node| node.value(&db)).collect::<Vec<_>>(),
        [10, 11]
    );
    db.clear_logs();

    input.set_use_previous(&mut db).to(true);
    let nodes = mixed_order_duplicate_nodes(&db, input);

    assert_eq!(
        nodes.iter().map(|node| node.value(&db)).collect::<Vec<_>>(),
        [20, 20, 11, 21]
    );
    db.assert_logs(expect![[r#"
        [
            "mixed_order_duplicate_nodes",
        ]"#]]);
}

#[test]
fn previous_liveness_is_not_permanent() {
    let mut db = DiscardLoggerDatabase::default();
    let input = Switch::new(&db, 1, false);

    assert_eq!(maybe_previous_node(&db, input).unwrap().value(&db), 1);
    db.clear_logs();

    input.set_use_previous(&mut db).to(true);
    assert_eq!(maybe_previous_node(&db, input).unwrap().value(&db), 1);
    db.assert_logs(expect![[r#"
        [
            "maybe_previous_node",
        ]"#]]);

    input.set_use_previous(&mut db).to(false);
    input.set_value(&mut db).to(0);
    assert!(maybe_previous_node(&db, input).is_none());
    db.assert_logs(expect![[r#"
        [
            "maybe_previous_node",
            "salsa_event(WillDiscardStaleOutput { execute_key: maybe_previous_node(Id(0)), output_key: NamedNode(Id(400)) })",
            "salsa_event(DidDiscard { key: NamedNode(Id(400)) })",
        ]"#]]);
}

#[test]
fn previous_replays_tracked_field_dependencies() {
    let mut db = LoggerDatabase::default();
    let switch = Switch::new(&db, 0, false);
    let input = Number::new(&db, 1);

    assert_eq!(read_source_or_previous(&db, switch, input), 1);
    db.clear_logs();

    switch.set_use_previous(&mut db).to(true);
    assert_eq!(read_source_or_previous(&db, switch, input), 1);
    db.clear_logs();

    input.set_value(&mut db).to(2);
    assert_eq!(read_source_or_previous(&db, switch, input), 1);
    db.assert_logs(expect![[r#"
        [
            "source_node",
            "read_source_or_previous",
        ]"#]]);
}

#[test]
#[should_panic(
    expected = "cannot access previous memoized value for previous_plus_one outside of its tracked function"
)]
fn previous_panics_outside_of_same_query() {
    let db = LoggerDatabase::default();
    previous_plus_one::previous(&db);
}
