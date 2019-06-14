#![cfg(test)]

use super::*;

#[derive(Debug)]
struct TestNode {
    data: usize,
    links: LruLinks<TestNode>,
}

impl TestNode {
    fn new(data: usize) -> Arc<Self> {
        Arc::new(TestNode {
            data,
            links: LruLinks::default(),
        })
    }
}

impl LruNode for TestNode {
    fn links(&self) -> &LruLinks<TestNode> {
        &self.links
    }
}

#[test]
fn queue() {
    let mut lru = Lru::default();
    let n1 = TestNode::new(1);
    let n2 = TestNode::new(2);
    let n3 = TestNode::new(3);

    assert!(lru.pop_lru().is_none());

    assert_eq!(lru.promote(n1.clone()), 1);
    assert_eq!(lru.promote(n2.clone()), 2);
    assert_eq!(lru.promote(n3.clone()), 3);

    assert!(Arc::ptr_eq(&n1, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n2, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n3, &lru.pop_lru().unwrap()));
    assert!(lru.pop_lru().is_none());
}

#[test]
fn promote_last() {
    let mut lru = Lru::default();
    let n1 = TestNode::new(1);
    let n2 = TestNode::new(2);
    let n3 = TestNode::new(3);

    assert_eq!(lru.promote(n1.clone()), 1);
    assert_eq!(lru.promote(n2.clone()), 2);
    assert_eq!(lru.promote(n3.clone()), 3);
    assert_eq!(lru.promote(n1.clone()), 3);

    assert!(Arc::ptr_eq(&n2, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n3, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n1, &lru.pop_lru().unwrap()));
    assert!(lru.pop_lru().is_none());
}

#[test]
fn promote_middle() {
    let mut lru = Lru::default();
    let n1 = TestNode::new(1);
    let n2 = TestNode::new(2);
    let n3 = TestNode::new(3);

    assert_eq!(lru.promote(n1.clone()), 1);
    assert_eq!(lru.promote(n2.clone()), 2);
    assert_eq!(lru.promote(n3.clone()), 3);
    assert_eq!(lru.promote(n2.clone()), 3);

    assert!(Arc::ptr_eq(&n1, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n3, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n2, &lru.pop_lru().unwrap()));
    assert!(&lru.pop_lru().is_none());
}

#[test]
fn promote_head() {
    let mut lru = Lru::default();
    let n1 = TestNode::new(1);
    let n2 = TestNode::new(2);
    let n3 = TestNode::new(3);

    assert_eq!(lru.promote(n1.clone()), 1);
    assert_eq!(lru.promote(n2.clone()), 2);
    assert_eq!(lru.promote(n3.clone()), 3);
    assert_eq!(lru.promote(n3.clone()), 3);

    assert!(Arc::ptr_eq(&n1, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n2, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n3, &lru.pop_lru().unwrap()));
    assert!(&lru.pop_lru().is_none());
}

#[test]
fn promote_rev() {
    let mut lru = Lru::default();
    let n1 = TestNode::new(1);
    let n2 = TestNode::new(2);
    let n3 = TestNode::new(3);

    assert_eq!(lru.promote(n1.clone()), 1);
    assert_eq!(lru.promote(n2.clone()), 2);
    assert_eq!(lru.promote(n3.clone()), 3);
    assert_eq!(lru.promote(n3.clone()), 3);
    assert_eq!(lru.promote(n2.clone()), 3);
    assert_eq!(lru.promote(n1.clone()), 3);

    assert!(Arc::ptr_eq(&n3, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n2, &lru.pop_lru().unwrap()));
    assert!(Arc::ptr_eq(&n1, &lru.pop_lru().unwrap()));
    assert!(&lru.pop_lru().is_none());
}
