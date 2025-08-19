#![cfg(feature = "inventory")]

mod common;

use crate::common::{EventLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{CycleRecoveryAction, Database, Setter};

#[derive(Clone, Debug, Eq, PartialEq, Hash, salsa::Update)]
struct Graph<'db> {
    nodes: Vec<Node<'db>>,
}

impl<'db> Graph<'db> {
    fn find_node(&self, db: &dyn salsa::Database, name: &str) -> Option<Node<'db>> {
        self.nodes
            .iter()
            .find(|node| node.name(db) == name)
            .copied()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Edge {
    // Index into `graph.nodes`
    to: usize,
    cost: usize,
}

#[salsa::tracked(debug)]
struct Node<'db> {
    #[returns(ref)]
    name: String,

    #[returns(deref)]
    #[tracked]
    edges: Vec<Edge>,

    graph: GraphInput,
}

#[salsa::input(debug)]
struct GraphInput {
    simple: bool,
    fixpoint_variant: usize,
}

#[salsa::tracked(returns(ref))]
fn create_graph(db: &dyn salsa::Database, input: GraphInput) -> Graph<'_> {
    if input.simple(db) {
        let a = Node::new(db, "a".to_string(), vec![], input);
        let b = Node::new(db, "b".to_string(), vec![Edge { to: 0, cost: 20 }], input);
        let c = Node::new(db, "c".to_string(), vec![Edge { to: 1, cost: 2 }], input);

        Graph {
            nodes: vec![a, b, c],
        }
    } else {
        // ```
        // flowchart TD
        //
        // A("a")
        // B("b")
        // C("c")
        // D{"d"}
        //
        // B -- 20 --> D
        // C -- 4 --> D
        // D -- 4 --> A
        // D -- 4 --> B
        // ```
        let a = Node::new(db, "a".to_string(), vec![], input);
        let b = Node::new(db, "b".to_string(), vec![Edge { to: 3, cost: 20 }], input);
        let c = Node::new(db, "c".to_string(), vec![Edge { to: 3, cost: 4 }], input);
        let d = Node::new(
            db,
            "d".to_string(),
            vec![Edge { to: 0, cost: 4 }, Edge { to: 1, cost: 4 }],
            input,
        );

        Graph {
            nodes: vec![a, b, c, d],
        }
    }
}

/// Computes the minimum cost from the node with offset `0` to the given node.
#[salsa::tracked(cycle_fn=cycle_recover, cycle_initial=max_initial)]
fn cost_to_start<'db>(db: &'db dyn Database, node: Node<'db>) -> usize {
    let mut min_cost = usize::MAX;
    let graph = create_graph(db, node.graph(db));

    for edge in node.edges(db) {
        if edge.to == 0 {
            min_cost = min_cost.min(edge.cost);
        }

        let edge_cost_to_start = cost_to_start(db, graph.nodes[edge.to]);

        // We hit a cycle, never take this edge because it will always be more expensive than
        // any other edge
        if edge_cost_to_start == usize::MAX {
            continue;
        }

        min_cost = min_cost.min(edge.cost + edge_cost_to_start);
    }

    min_cost
}

fn max_initial(_db: &dyn Database, _node: Node) -> usize {
    usize::MAX
}

fn cycle_recover(
    _db: &dyn Database,
    _value: &usize,
    _count: u32,
    _inputs: Node,
) -> CycleRecoveryAction<usize> {
    CycleRecoveryAction::Iterate
}

/// Tests for cycles where the cycle head is stored on a tracked struct
/// and that tracked struct is freed in a later revision.
#[test]
fn main() {
    let mut db = EventLoggerDatabase::default();

    let input = GraphInput::new(&db, false, 0);
    let graph = create_graph(&db, input);
    let c = graph.find_node(&db, "c").unwrap();

    // Query the cost from `c` to `a`.
    // There's a cycle between `b` and `d`, where `d` becomes the cycle head and `b` is a provisional, non finalized result.
    assert_eq!(cost_to_start(&db, c), 8);

    // Change the graph, this will remove `d`, leaving `b` pointing to a cycle head that's now collected.
    // Querying the cost from `c` to `a` should try to verify the result of `b` and it is important
    // that `b` doesn't try to dereference the cycle head (because its memo is now stored on a tracked
    // struct that has been freed).
    input.set_simple(&mut db).to(true);

    let graph = create_graph(&db, input);
    let c = graph.find_node(&db, "c").unwrap();

    assert_eq!(cost_to_start(&db, c), 22);

    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: create_graph(Id(0)) }",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(402)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(403)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(400)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(401)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: cost_to_start(Id(403)), iteration_count: IterationCount(1), fell_back: false }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(401)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: create_graph(Id(0)) }",
            "WillDiscardStaleOutput { execute_key: create_graph(Id(0)), output_key: Node(Id(403)) }",
            "DidDiscard { key: Node(Id(403)) }",
            "DidDiscard { key: cost_to_start(Id(403)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(402)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(401)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: cost_to_start(Id(400)) }",
            "WillCheckCancellation",
        ]"#]]);
}

#[salsa::tracked]
struct IterationNode<'db> {
    #[returns(ref)]
    name: String,
    iteration: usize,
}

/// A cyclic query that creates more tracked structs in later fixpoint iterations.
///
/// The output depends on the input's fixpoint_variant:
/// - variant=0: Returns `[base]` (1 struct, no cycle)
/// - variant=1: Through fixpoint iteration, returns `[iter_0, iter_1, iter_2]` (3 structs)
/// - variant=2: Through fixpoint iteration, returns `[iter_0, iter_1]` (2 structs)
/// - variant>2: Through fixpoint iteration, returns `[iter_0, iter_1]` (2 structs, same as variant=2)
///
/// When variant > 0, the query creates a cycle by calling itself. The fixpoint iteration
/// proceeds as follows:
/// 1. Initial: returns empty vector
/// 2. First iteration: returns `[iter_0]`
/// 3. Second iteration: returns `[iter_0, iter_1]`
/// 4. Third iteration (only for variant=1): returns `[iter_0, iter_1, iter_2]`
/// 5. Further iterations: no change, fixpoint reached
#[salsa::tracked(cycle_fn=cycle_recover_with_structs, cycle_initial=initial_with_structs)]
fn create_tracked_in_cycle<'db>(
    db: &'db dyn Database,
    input: GraphInput,
) -> Vec<IterationNode<'db>> {
    // Check if we should create more nodes based on the input.
    let variant = input.fixpoint_variant(db);

    if variant == 0 {
        // Base case - no cycle, just return a single node.
        vec![IterationNode::new(db, "base".to_string(), 0)]
    } else {
        // Create a cycle by calling ourselves.
        let previous = create_tracked_in_cycle(db, input);

        // In later iterations, create additional tracked structs.
        if previous.is_empty() {
            // First iteration - initial returns empty.
            vec![IterationNode::new(db, "iter_0".to_string(), 0)]
        } else {
            // Limit based on variant: variant=1 allows 3 nodes, variant=2 allows 2 nodes.
            let limit = if variant == 1 { 3 } else { 2 };

            if previous.len() < limit {
                // Subsequent iterations - add more nodes.
                let mut nodes = previous;
                nodes.push(IterationNode::new(
                    db,
                    format!("iter_{}", nodes.len()),
                    nodes.len(),
                ));
                nodes
            } else {
                // Reached the limit.
                previous
            }
        }
    }
}

fn initial_with_structs(_db: &dyn Database, _input: GraphInput) -> Vec<IterationNode<'_>> {
    vec![]
}

fn cycle_recover_with_structs<'db>(
    _db: &'db dyn Database,
    _value: &Vec<IterationNode<'db>>,
    _iteration: u32,
    _input: GraphInput,
) -> CycleRecoveryAction<Vec<IterationNode<'db>>> {
    CycleRecoveryAction::Iterate
}

#[test]
fn test_cycle_with_fixpoint_structs() {
    let mut db = EventLoggerDatabase::default();

    // Create an input that will trigger the cyclic behavior.
    let input = GraphInput::new(&db, false, 1);

    // Initial query - this will create structs across multiple iterations.
    let nodes = create_tracked_in_cycle(&db, input);
    assert_eq!(nodes.len(), 3);
    // First iteration: previous is empty [], so we get [iter_0]
    // Second iteration: previous is [iter_0], so we get [iter_0, iter_1]
    // Third iteration: previous is [iter_0, iter_1], so we get [iter_0, iter_1, iter_2]
    assert_eq!(nodes[0].name(&db), "iter_0");
    assert_eq!(nodes[1].name(&db), "iter_1");
    assert_eq!(nodes[2].name(&db), "iter_2");

    // Clear logs to focus on the change.
    db.clear_logs();

    // Change the input to force re-execution with a different variant.
    // This will create 2 tracked structs instead of 3 (one fewer than before).
    input.set_fixpoint_variant(&mut db).to(2);

    // Re-query - this should handle the tracked struct changes properly.
    let nodes = create_tracked_in_cycle(&db, input);
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].name(&db), "iter_0");
    assert_eq!(nodes[1].name(&db), "iter_1");

    // Check the logs to ensure proper execution and struct management.
    // We should see the third struct (iter_2) being discarded.
    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: create_tracked_in_cycle(Id(0)) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: create_tracked_in_cycle(Id(0)), iteration_count: IterationCount(1), fell_back: false }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: create_tracked_in_cycle(Id(0)), iteration_count: IterationCount(2), fell_back: false }",
            "WillCheckCancellation",
            "WillDiscardStaleOutput { execute_key: create_tracked_in_cycle(Id(0)), output_key: IterationNode(Id(402)) }",
            "DidDiscard { key: IterationNode(Id(402)) }",
        ]"#]]);
}

// Additional test structures for the new scenario
#[salsa::tracked]
struct TrackedValue<'db> {
    value: u32,
}

#[salsa::input]
struct InputValue {
    value: u32,
}

#[salsa::input]
struct IterationCounter {
    count: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

#[salsa::tracked]
fn query_c<'db>(db: &'db dyn Database, tracked: TrackedValue<'db>) -> u32 {
    tracked.value(db)
}

#[salsa::tracked(cycle_fn=cycle_recover_b, cycle_initial=initial_b)]
fn query_b<'db>(db: &'db dyn Database, input: InputValue) -> u32 {
    // Call query_a to create the cycle
    let a_result = query_a(db, input);
    
    // Only create tracked struct when a_result reaches a certain threshold
    // This creates an internal condition for when to create the tracked struct
    if a_result <= 50 {
        let tracked = TrackedValue::new(db, 42);
        let c_result = query_c(db, tracked);
        c_result
    } else {
        a_result - 10  // Reduce by 10 to force iteration
    }
}

fn initial_b(_db: &dyn Database, _input: InputValue) -> u32 {
    u32::MAX
}

fn cycle_recover_b(
    _db: &dyn Database,
    _value: &u32,
    _count: u32,
    _input: InputValue,
) -> CycleRecoveryAction<u32> {
    CycleRecoveryAction::Iterate
}

#[salsa::tracked(cycle_fn=cycle_recover_a, cycle_initial=initial_a)]
fn query_a<'db>(db: &'db dyn Database, input: InputValue) -> u32 {
    let input_val = input.value(db);
    // Call query_b to create the cycle
    let b_result = query_b(db, input);
    b_result.min(input_val)
}

fn initial_a(_db: &dyn Database, _input: InputValue) -> u32 {
    u32::MAX
}

fn cycle_recover_a(
    _db: &dyn Database,
    _value: &u32,
    _count: u32,
    _input: InputValue,
) -> CycleRecoveryAction<u32> {
    CycleRecoveryAction::Iterate
}

/// Test scenario with tracked struct created during cycle iteration.
///
/// a -> b -> a (cycle)
///        -> c(tracked_struct)
///
/// - a is the cycle head
/// - b participates in the cycle and creates a tracked struct based on internal condition
/// - The tracked struct is created when a_result <= 50 (internal condition, not explicit counter)
/// - When input changes, a must rerun and should panic due to tracked struct cleanup issue
#[test]
#[should_panic(expected = "cannot delete read-locked id")]
fn cycle_with_tracked_struct_creation_during_iteration() {
    let mut db = EventLoggerDatabase::default();
    
    // Set up inputs
    let input = InputValue::new(&db, 50);
    
    // Execute query_a which triggers the cycle
    let result = query_a(&db, input);
    
    // First iteration: a returns input (50), b returns a_result - 10 = 40
    // Second iteration: a returns min(50, 40) = 40, b sees a_result <= 50, creates tracked struct
    // Result should be the tracked struct value (42)
    assert_eq!(result, 42);
    
    // Clear logs for the next part
    db.clear_logs();
    
    // Change the input to force recomputation
    input.set_value(&mut db).to(30);
    
    // Re-execute, should see appropriate logs
    let result2 = query_a(&db, input);
    
    // New result should be 42 (tracked struct value), but this should panic first
    assert_eq!(result2, 42);
    
    // Verify we see the expected execution pattern in logs
    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: query_a(Id(0)) }",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: query_b(Id(0)) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(1), fell_back: false }",
            "WillCheckCancellation",
            "WillExecute { database_key: query_b(Id(0)) }",
            "WillCheckCancellation",
        ]"#]]);
}
