//! Regression for a thread being made to wait on itself during cycle recovery.
//!
//! A, B, C, and D name the `reachable` queries for the four input nodes. Initially
//! their calls form the cycle A -> B -> D -> A, and C calls A from outside that
//! cycle. The edit adds a call B -> C, so C becomes part of the cycle too.
//!
//! Four threads start by querying A, B, C, and D respectively. Call them T_A,
//! T_B, T_C, and T_D. One failing interleaving after the edit is:
//!
//! 1. A transfers its lock to B: A stays locked, and B is responsible for
//!    eventually releasing it. While T_B waits for D, T_D reentrantly claims A.
//! 2. T_C requests A during that reentrant claim, so T_C waits on T_D.
//! 3. T_D releases its reentrant claim on A, then transfers D's lock to B and
//!    waits on T_B. A's lock belongs to B again, but T_C's wait still points at
//!    T_D. The thread waits are now T_C -> T_D -> T_B.
//! 4. B transfers its lock, including the locks for A and D, to C. Salsa wakes
//!    T_D, then redirects the remaining waiters for those locks to T_C. But
//!    T_C is itself still waiting for A! Redirecting that wait creates T_C -> T_C.
//!
//! The test checks that every reader finishes with the converged value. Query
//! cycles are supported here; a cycle in the thread wait graph would deadlock.

use salsa::{Database, DatabaseImpl, Setter};

use crate::sync::{Arc, Barrier, thread};

#[salsa::input]
struct Node {
    #[returns(ref)]
    successors: Vec<Node>,
    #[returns(copy)]
    value: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn reachable(db: &dyn Database, node: Node) -> u32 {
    let mut value = node.value(db);
    for &successor in node.successors(db) {
        value |= reachable(db, successor);
    }
    value
}

#[test_log::test]
fn ownership_transfer_does_not_block_its_new_owner_on_itself() {
    crate::sync::check(|| {
        let mut db = DatabaseImpl::default();
        let a = Node::new(&db, Vec::new(), 1);
        let b = Node::new(&db, Vec::new(), 2);
        let c = Node::new(&db, Vec::new(), 4);
        let d = Node::new(&db, Vec::new(), 8);

        a.set_successors(&mut db).to(vec![b]);
        b.set_successors(&mut db).to(vec![d]);
        c.set_successors(&mut db).to(vec![a]);
        d.set_successors(&mut db).to(vec![a]);

        assert_eq!(read_concurrently(&db, [a, b, c, d]), [11, 11, 15, 11]);

        b.set_successors(&mut db).to(vec![d, c]);

        assert_eq!(read_concurrently(&db, [a, b, c, d]), [15; 4]);
    });
}

fn read_concurrently(db: &DatabaseImpl, nodes: [Node; 4]) -> [u32; 4] {
    let barrier = Arc::new(Barrier::new(4));
    let readers = nodes.map(|node| {
        let db = db.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            reachable(&db, node)
        })
    });
    readers.map(|reader| reader.join().unwrap())
}
