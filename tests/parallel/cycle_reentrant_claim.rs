//! A reader waiting on a reentrant claim must still make progress when that claim ends.
//!
//! A, B, C, and D name the `reachable` queries for four input nodes. Initially
//! A calls B, B calls D, and D calls A. C also calls A, from outside that cycle.
//! The edit adds a call from D to C, making C part of the cycle.
//!
//! After A transfers its lock to B, the thread evaluating D can reentrantly
//! claim A. The thread evaluating C can then block on that reentrant claim.
//! When the claim ends, C's wait must point at B's thread again.
//! Otherwise, its wait still points at D's thread. This can let D's thread
//! return an unfinished result while the other three readers remain blocked,
//! even if ownership transfers wake every affected waiter.

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
fn releasing_reentrant_claim_preserves_waiter_progress() {
    let test = || {
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

        d.set_successors(&mut db).to(vec![a, c]);

        assert_eq!(read_concurrently(&db, [a, b, c, d]), [15; 4]);
    };

    #[cfg(not(feature = "shuttle"))]
    test();

    #[cfg(feature = "shuttle")]
    {
        // Keep the schedule search reproducible: this seed exercises the stale wait edge.
        let scheduler = shuttle::scheduler::PctScheduler::new_from_seed(0, 50, 2500);
        let mut config = shuttle::Config::default();
        config.stack_size = 1024 * 1024;
        shuttle::Runner::new(scheduler, config).run(test);
    }
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
