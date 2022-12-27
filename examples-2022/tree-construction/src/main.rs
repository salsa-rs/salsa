/// This example is an analogy of a calculation
/// that has multiple inputs that refer to each other
/// and any input can be the entry point of a calculation.
///
/// `FlatNode` represents an input.
/// `Tree` represents an intermediate or final result of a computation.
/// `construct_tree` represents an internal recursive computation.
/// `entrypoint` represents an entrypoint of a computation.

#[salsa::jar(db = Db)]
struct Jar(FlatNode, Tree, construct_tree, entrypoint);

trait Db: salsa::DbWithJar<Jar> {}

impl<DB> Db for DB where DB: ?Sized + salsa::DbWithJar<Jar> {}

#[derive(Default)]
#[salsa::db(Jar)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

// Looks like recursive but it's not.
#[salsa::input]
struct FlatNode {
    #[salsa::id]
    id: usize,
    children: Vec<FlatNode>,
}

#[salsa::tracked]
struct Tree {
    root: Node,
}

// True recursive data structure.
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
            .iter()
            .map(|child| construct_tree(db, *child).root(db))
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

    let node0 = FlatNode::new(&db, 0, vec![]);
    let node1 = FlatNode::new(&db, 1, vec![]);
    let node2 = FlatNode::new(&db, 2, vec![node0, node1]);
    let node3 = FlatNode::new(&db, 3, vec![node2]);

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

    node2.set_children(&mut db).to(vec![node1]);
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

    // TODO: How to remove a node?
    // node0.remove(&mut db);
}
