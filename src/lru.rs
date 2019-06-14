use arc_swap::ArcSwapOption;
use arc_swap::Lease;
use std::fmt::Debug;
use std::sync::Arc;

mod test;

/// A very simple concurrent lru list, built using a doubly linked
/// list of Arcs.
///
/// The list uses a very simple locking scheme and will probably
/// suffer under high contention. This could certainly be improved.
///
/// We assume but do not verify that each node is only used with one
/// list. If this is not the case, it is not *unsafe*, but panics and
/// weird results will ensue.
///
/// Each "node" in the list is of type `Node` and must implement
/// `LruNode`, which is a trait that gives access to a field of type
/// `LruLinks<Node>`, which stores the prev/next points.
#[derive(Debug)]
pub(crate) struct Lru<Node>
where
    Node: LruNode,
{
    len: usize,
    head: Option<Arc<Node>>,
    tail: Option<Arc<Node>>,
}

pub(crate) trait LruNode: Sized + Debug {
    fn links(&self) -> &LruLinks<Self>;
}

pub(crate) struct LruLinks<Node> {
    prev: ArcSwapOption<Node>,
    next: ArcSwapOption<Node>,
}

impl<Node> Default for Lru<Node>
where
    Node: LruNode,
{
    fn default() -> Self {
        Lru {
            len: 0,
            head: None,
            tail: None,
        }
    }
}

impl<Node> Drop for Lru<Node>
where
    Node: LruNode,
{
    fn drop(&mut self) {
        self.clear();
    }
}

impl<Node> Lru<Node>
where
    Node: LruNode,
{
    /// Removes everyting from the list.
    pub fn clear(&mut self) {
        // Not terribly efficient at the moment.
        while self.pop_lru().is_some() {}
    }

    /// Removes the least-recently-used item in the list.
    pub fn pop_lru(&mut self) -> Option<Arc<Node>> {
        log::debug!("pop_lru(self={:?})", self);
        let node = self.tail.take()?;
        debug_assert!(node.links().next.load().is_none());
        self.tail = node.links().prev.swap(None);
        if let Some(new_tail) = &self.tail {
            new_tail.links().next.store(None);
            self.len -= 1;
        } else {
            self.head = None;
        }
        Some(node)
    }

    /// Makes `node` the least-recently-used item in the list, adding
    /// it to the list if it was not already a member.
    pub fn promote(&mut self, node: Arc<Node>) -> usize {
        log::debug!("promote(node={:?})", node);
        let node = node.clone();

        let node_links = node.links();

        // First: check if the node is already in the linked list and has neighbors.
        // If so, let's unlink it.
        {
            let old_prev = node_links.prev.lease().into_option();
            let old_next = node_links.next.lease().into_option();
            log::debug!("promote: old_prev={:?}", old_prev);
            log::debug!("promote: old_next={:?}", old_next);
            match (old_prev, old_next) {
                (Some(old_prev), Some(old_next)) => {
                    // Node is in the middle of the list.
                    old_prev.links().next.store(Some(Lease::upgrade(&old_next)));
                    old_next
                        .links()
                        .prev
                        .store(Some(Lease::into_upgrade(old_prev)));
                    self.len -= 1;
                }
                (None, Some(_)) => {
                    // Node is already at the head of the list. Nothing to do here.
                    return self.len;
                }
                (Some(old_prev), None) => {
                    // Node is at the tail of the (non-empty) list.
                    old_prev.links().next.store(None);
                    self.tail = Some(Lease::into_upgrade(old_prev));
                    self.len -= 1;
                }
                (None, None) => {
                    // Node is either not in the list *or* at the head of a singleton list.
                    if let Some(head) = &self.head {
                        if Arc::ptr_eq(head, &node) {
                            // Node is at the head.
                            return self.len;
                        }
                    }
                }
            }
        }

        // At this point, the node's links are stale but the node is not a member
        // of the list.
        let current_head: Option<Arc<Node>> = self.head.clone();
        if let Some(current_head) = &current_head {
            current_head.links().prev.store(Some(node.clone()));
        }
        node_links.next.store(current_head);
        node_links.prev.store(None);
        if self.len == 0 {
            self.tail = Some(node.clone());
        }
        self.head = Some(node);
        self.len += 1;
        return self.len;
    }
}

impl<Node> Default for LruLinks<Node> {
    fn default() -> Self {
        Self {
            prev: ArcSwapOption::default(),
            next: ArcSwapOption::default(),
        }
    }
}

impl<Node> std::fmt::Debug for LruLinks<Node> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "LruLinks {{ .. }}")
    }
}
