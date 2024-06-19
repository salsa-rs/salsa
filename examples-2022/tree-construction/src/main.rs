// This example is an analogy of a calculation
// that has multiple inputs that depend on each other
// and any input can be the entry point of a calculation.
//
// - `FlatNode` represents an input.
// - `usize` represents an input id.
// - `Tree` represents an intermediate or final result of a computation.
// - `fn construct_tree` represents an internal recursive computation.
// - `fn entrypoint` represents an entrypoint of a computation.

#[salsa::jar(db = Db)]
struct Jar(FlatNode, Tree, construct_tree, entrypoint);

trait Db: salsa::DbWithJar<Jar> {
    fn create_flat_node(&mut self, id: usize, children: Vec<usize>) -> FlatNode;

    fn get_flat_node(&self, id: usize) -> FlatNode;

    fn remove_flat_node(&mut self, id: usize);
}

impl Db for Database {
    fn create_flat_node(&mut self, id: usize, children: Vec<usize>) -> FlatNode {
        let flat_node = FlatNode::new(self, id, children);
        self.flat_nodes.insert(id, flat_node);
        flat_node
    }

    fn get_flat_node(&self, id: usize) -> FlatNode {
        self.flat_nodes[&id]
    }

    fn remove_flat_node(&mut self, id: usize) {
        let _flat_node = self.flat_nodes.remove(&id);
        // TODO: How to remove it from the storage?
    }
}

#[derive(Default)]
#[salsa::db(Jar)]
struct Database {
    storage: salsa::Storage<Self>,
    flat_nodes: std::collections::HashMap<usize, FlatNode>,
}

impl salsa::Database for Database {}

#[salsa::input]
struct FlatNode {
    #[salsa::id]
    id: usize,
    children: Vec<usize>,
}

// We cannot construct a real recursive data structure as a salsa item,
// so we need a wrapper.
#[salsa::tracked]
struct Tree {
    root: Node,
}

// The recursive data structure.
#[derive(Debug, PartialEq, Eq, Clone)]
struct Node {
    id: usize,
    children: Vec<Node>,
}

#[salsa::tracked]
fn construct_tree(db: &dyn Db, node: FlatNode) -> Tree {
    let node = Node {
        id: node.id(db),
        children: node
            .children(db)
            .into_iter()
            .map(|child| {
                let flat_node = db.get_flat_node(child);
                let tree = construct_tree(db, flat_node);
                // Unwrap the wrapper.
                tree.root(db)
            })
            .collect(),
    };
    Tree::new(db, node)
}

#[salsa::tracked]
fn entrypoint(db: &dyn Db, node: FlatNode) -> Tree {
    construct_tree(db, node)
}

pub fn main() {
    let mut db = Database::default();

    let node0 = db.create_flat_node(0, vec![]);
    let _node1 = db.create_flat_node(1, vec![]);
    let node2 = db.create_flat_node(2, vec![0, 1]);
    let node3 = db.create_flat_node(3, vec![2]);

    assert_eq!(
        entrypoint(&db, node0).root(&db),
        Node {
            id: 0,
            children: vec![]
        }
    );
    assert_eq!(
        entrypoint(&db, node3).root(&db),
        Node {
            id: 3,
            children: vec![Node {
                id: 2,
                children: vec![
                    Node {
                        id: 0,
                        children: vec![]
                    },
                    Node {
                        id: 1,
                        children: vec![]
                    }
                ]
            }]
        }
    );

    // Removes nede0 from node2's children.
    node2.set_children(&mut db).to(vec![1]);
    assert_eq!(
        entrypoint(&db, node3).root(&db),
        Node {
            id: 3,
            children: vec![Node {
                id: 2,
                children: vec![Node {
                    id: 1,
                    children: vec![]
                }]
            }]
        }
    );

    db.remove_flat_node(0)
}
