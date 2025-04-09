//! Tests for cycles where the cycle head is stored on a tracked struct.

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
    #[return_ref]
    name: String,

    #[return_ref]
    #[tracked]
    edges: Vec<Edge>,

    graph: GraphInput,
}

#[salsa::input(debug)]
struct GraphInput {
    simple: bool,
}

#[salsa::tracked(return_ref)]
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
    let mut min_cost = None;
    let graph = create_graph(db, node.graph(db));

    for edge in node.edges(db) {
        if edge.to == 0 {
            if min_cost.is_none_or(|min| min > edge.cost) {
                min_cost = Some(edge.cost);
            }
        }

        let edge_cost_to_start = cost_to_start(db, graph.nodes[edge.to]);

        // We hit a cycle, never take this edge because it will always be more expensive than
        // any other edge
        if edge_cost_to_start == usize::MAX {
            continue;
        }

        let total_cost = edge.cost + edge_cost_to_start;
        if min_cost.is_none_or(|min| min > total_cost) {
            min_cost = Some(total_cost);
        }
    }

    // If `None`, it means that there's no path from `node` to the start.
    min_cost.unwrap_or(usize::MAX)
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

#[test]
fn main() {
    let mut db = EventLoggerDatabase::default();

    let input = GraphInput::new(&mut db, false);
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

    db.assert_logs(expect![[r#""#]]);
}
