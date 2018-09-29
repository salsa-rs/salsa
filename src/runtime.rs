use crate::Query;
use crate::QueryContext;
use rustc_hash::FxHasher;
use std::cell::RefCell;
use std::fmt::Write;
use std::hash::BuildHasherDefault;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

type FxIndexSet<K> = indexmap::IndexSet<K, BuildHasherDefault<FxHasher>>;

/// The salsa runtime stores the storage for all queries as well as
/// tracking the query stack and dependencies between cycles.
///
/// Each new runtime you create (e.g., via `Runtime::new` or
/// `Runtime::default`) will have an independent set of query storage
/// associated with it. Normally, therefore, you only do this once, at
/// the start of your application.
pub struct Runtime<QC: QueryContext> {
    shared_state: Arc<SharedState<QC>>,
    local_state: RefCell<LocalState<QC>>,
}

impl<QC> Default for Runtime<QC>
where
    QC: QueryContext,
{
    fn default() -> Self {
        Runtime {
            shared_state: Arc::new(SharedState {
                storage: Default::default(),
                revision: Default::default(),
            }),
            local_state: RefCell::new(LocalState {
                query_stack: Default::default(),
            }),
        }
    }
}

impl<QC> Runtime<QC>
where
    QC: QueryContext,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn storage(&self) -> &QC::QueryContextStorage {
        &self.shared_state.storage
    }

    /// Read current value of the revision counter.
    crate fn current_revision(&self) -> Revision {
        Revision {
            generation: self.shared_state.revision.load(Ordering::SeqCst),
        }
    }

    /// Increments the current revision counter and returns the new value.
    crate fn increment_revision(&self) -> Revision {
        Revision {
            generation: 1 + self.shared_state.revision.fetch_add(1, Ordering::SeqCst),
        }
    }

    crate fn execute_query_implementation<Q>(
        &self,
        query: &QC,
        descriptor: QC::QueryDescriptor,
        key: &Q::Key,
    ) -> Q::Value
    where
        Q: Query<QC>,
    {
        self.local_state
            .borrow_mut()
            .query_stack
            .push(ActiveQuery::new(descriptor));
        let value = Q::execute(query, key.clone());
        self.local_state.borrow_mut().query_stack.pop();
        value
    }

    /// Obviously, this should be user configurable at some point.
    crate fn report_unexpected_cycle(&self, descriptor: QC::QueryDescriptor) -> ! {
        let local_state = self.local_state.borrow();
        let LocalState { query_stack, .. } = &*local_state;

        let start_index = (0..query_stack.len())
            .rev()
            .filter(|&i| query_stack[i].descriptor == descriptor)
            .next()
            .unwrap();

        let mut message = format!("Internal error, cycle detected:\n");
        for active_query in &query_stack[start_index..] {
            writeln!(message, "- {:?}\n", active_query.descriptor).unwrap();
        }
        panic!(message)
    }
}

/// State that will be common to all threads (when we support multiple threads)
struct SharedState<QC: QueryContext> {
    storage: QC::QueryContextStorage,
    revision: AtomicU64,
}

/// State that will be specific to a single execution threads (when we support multiple threads)
struct LocalState<QC: QueryContext> {
    query_stack: Vec<ActiveQuery<QC>>,
}

struct ActiveQuery<QC: QueryContext> {
    /// What query is executing
    descriptor: QC::QueryDescriptor,

    /// Each time we execute a subquery, it returns to us the revision
    /// in which its value last changed. We track the maximum of these
    /// to find the maximum revision in which *we* changed.
    max_revision_read: Revision,

    /// Each subquery
    subqueries: FxIndexSet<QC::QueryDescriptor>,
}

impl<QC: QueryContext> ActiveQuery<QC> {
    fn new(descriptor: QC::QueryDescriptor) -> Self {
        ActiveQuery {
            descriptor,
            max_revision_read: Revision::zero(),
            subqueries: FxIndexSet::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: u64,
}

impl Revision {
    crate fn zero() -> Self {
        Revision { generation: 0 }
    }
}
