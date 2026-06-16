//! Incremental validation across common dependency graph shapes.

use std::hint::black_box;

use codspeed_criterion_compat::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};
use salsa::Setter;

const GRAPH_SIZE: usize = 1_024;

#[salsa::input]
struct Node {
    value: usize,

    #[returns(ref)]
    dependencies: Vec<Node>,
}

#[salsa::tracked]
#[inline(never)]
fn evaluate(db: &dyn salsa::Database, node: Node) -> usize {
    node.dependencies(db)
        .iter()
        .fold(node.value(db) / 2, |result, &dependency| {
            result.wrapping_add(evaluate(db, dependency))
        })
}

fn new_nodes(db: &salsa::DatabaseImpl) -> Vec<Node> {
    (0..GRAPH_SIZE)
        .map(|index| Node::new(db, if index + 1 == GRAPH_SIZE { 2 } else { 0 }, Vec::new()))
        .collect()
}

#[derive(Clone, Copy)]
enum Shape {
    Chain,
    Fanout,
    Diamond,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Shape::Chain => "chain",
            Shape::Fanout => "fanout",
            Shape::Diamond => "diamond",
        }
    }
}

#[derive(Clone, Copy)]
enum Revision {
    Unrelated,
    Backdated,
    Changed,
}

impl Revision {
    fn name(self) -> &'static str {
        match self {
            Revision::Unrelated => "unrelated_revision",
            Revision::Backdated => "backdated_leaf",
            Revision::Changed => "changed_leaf",
        }
    }
}

struct GraphFixture {
    db: salsa::DatabaseImpl,
    root: Node,
    leaf: Node,
    unrelated: Node,
    initial_result: usize,
}

impl GraphFixture {
    fn new(shape: Shape) -> Self {
        let mut db = salsa::DatabaseImpl::new();

        let (root, leaf) = match shape {
            Shape::Chain => {
                let nodes = new_nodes(&db);
                for index in 0..GRAPH_SIZE - 1 {
                    nodes[index]
                        .set_dependencies(&mut db)
                        .to(vec![nodes[index + 1]]);
                }
                (nodes[0], nodes[GRAPH_SIZE - 1])
            }
            Shape::Fanout => {
                let leaves = new_nodes(&db);
                let root = Node::new(&db, 0, leaves.clone());
                (root, leaves[GRAPH_SIZE - 1])
            }
            Shape::Diamond => {
                let leaf = Node::new(&db, 2, Vec::new());
                let branches: Vec<_> = (0..GRAPH_SIZE)
                    .map(|_| Node::new(&db, 0, vec![leaf]))
                    .collect();
                (Node::new(&db, 0, branches), leaf)
            }
        };

        let unrelated = Node::new(&db, 0, Vec::new());
        let initial_result = evaluate(&db, root);

        Self {
            db,
            root,
            leaf,
            unrelated,
            initial_result,
        }
    }

    fn run(&mut self, revision: Revision) {
        match revision {
            Revision::Unrelated => {
                self.unrelated
                    .set_value(black_box(&mut self.db))
                    .to(black_box(1));
            }
            Revision::Backdated => {
                // `evaluate` divides the value by two, so 2 -> 3 preserves the result.
                self.leaf
                    .set_value(black_box(&mut self.db))
                    .to(black_box(3));
            }
            Revision::Changed => {
                self.leaf
                    .set_value(black_box(&mut self.db))
                    .to(black_box(4));
            }
        }

        let result = evaluate(black_box(&self.db), black_box(self.root));
        match revision {
            Revision::Unrelated | Revision::Backdated => {
                assert_eq!(black_box(result), self.initial_result);
            }
            Revision::Changed => assert_ne!(black_box(result), self.initial_result),
        }
    }
}

fn graph_shapes(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("Graph shapes");

    for shape in [Shape::Chain, Shape::Fanout, Shape::Diamond] {
        for revision in [Revision::Unrelated, Revision::Backdated, Revision::Changed] {
            group.bench_function(
                BenchmarkId::new(format!("{}/{}", shape.name(), revision.name()), GRAPH_SIZE),
                move |b| {
                    b.iter_batched_ref(
                        || GraphFixture::new(shape),
                        |fixture| fixture.run(revision),
                        BatchSize::LargeInput,
                    );
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, graph_shapes);
criterion_main!(benches);
