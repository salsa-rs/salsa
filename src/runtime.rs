use crate::Query;
use crate::QueryContext;
use log::debug;
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

    /// Indicates that some input to the system has changed and hence
    /// that memoized values **may** be invalidated. This cannot be
    /// invoked while query computation is in progress.
    ///
    /// As a user of the system, you would not normally invoke this
    /// method directly. Instead, you would use "input" queries and
    /// invoke their `set` method. But it can be useful if you have a
    /// "volatile" input that you must poll from time to time; in that
    /// case, you can wrap the input with a "no-storage" query and
    /// invoke this method from time to time.
    pub fn next_revision(&self) {
        if !self.local_state.borrow().query_stack.is_empty() {
            panic!("next_revision invoked during a query computation");
        }

        self.increment_revision();
    }

    /// Read current value of the revision counter.
    crate fn current_revision(&self) -> Revision {
        Revision {
            generation: self.shared_state.revision.load(Ordering::SeqCst),
        }
    }

    /// Increments the current revision counter and returns the new value.
    crate fn increment_revision(&self) -> Revision {
        let result = Revision {
            generation: 1 + self.shared_state.revision.fetch_add(1, Ordering::SeqCst),
        };

        debug!("increment_revision: incremented to {:?}", result);

        result
    }

    crate fn execute_query_implementation<Q>(
        &self,
        query: &QC,
        descriptor: &QC::QueryDescriptor,
        key: &Q::Key,
    ) -> (Q::Value, QueryDescriptorSet<QC>)
    where
        Q: Query<QC>,
    {
        debug!("{:?}({:?}): executing query", Q::default(), key);

        // Push the active query onto the stack.
        let push_len = {
            let mut local_state = self.local_state.borrow_mut();
            local_state
                .query_stack
                .push(ActiveQuery::new(descriptor.clone()));
            local_state.query_stack.len()
        };

        // Execute user's code, accumulating inputs etc.
        let value = Q::execute(query, key.clone());

        // Extract accumulated inputs.
        let ActiveQuery { subqueries, .. } = {
            let mut local_state = self.local_state.borrow_mut();

            // Sanity check: pushes and pops should be balanced.
            assert_eq!(local_state.query_stack.len(), push_len);

            local_state.query_stack.pop().unwrap()
        };

        (value, subqueries)
    }

    /// Reports that the currently active query read the result from
    /// another query.
    ///
    /// # Parameters
    ///
    /// - `descriptor`: the query whose result was read
    /// - `changed_revision`: the last revision in which the result of that
    ///   query had changed
    crate fn report_query_read(&self, descriptor: &QC::QueryDescriptor) {
        if let Some(top_query) = self.local_state.borrow_mut().query_stack.last_mut() {
            top_query.add_read(descriptor);
        }
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

/// State that will be specific to a single execution threads (when we
/// support multiple threads)
struct LocalState<QC: QueryContext> {
    query_stack: Vec<ActiveQuery<QC>>,
}

struct ActiveQuery<QC: QueryContext> {
    /// What query is executing
    descriptor: QC::QueryDescriptor,

    /// Each subquery
    subqueries: QueryDescriptorSet<QC>,
}

impl<QC: QueryContext> ActiveQuery<QC> {
    fn new(descriptor: QC::QueryDescriptor) -> Self {
        ActiveQuery {
            descriptor,
            subqueries: QueryDescriptorSet::new(),
        }
    }

    fn add_read(&mut self, subquery: &QC::QueryDescriptor) {
        self.subqueries.insert(subquery.clone());
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: u64,
}

impl Revision {
    crate const ZERO: Self = Revision { generation: 0 };
}

impl std::fmt::Debug for Revision {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "R{}", self.generation)
    }
}

/// An insertion-order-preserving set of queries. Used to track the
/// inputs accessed during query execution.
crate struct QueryDescriptorSet<QC: QueryContext> {
    set: FxIndexSet<QC::QueryDescriptor>,
}

impl<QC: QueryContext> std::fmt::Debug for QueryDescriptorSet<QC> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.set, fmt)
    }
}

impl<QC: QueryContext> QueryDescriptorSet<QC> {
    fn new() -> Self {
        QueryDescriptorSet {
            set: FxIndexSet::default(),
        }
    }

    /// Add `descriptor` to the set. Returns true if `descriptor` is
    /// newly added and false if `descriptor` was already a member.
    fn insert(&mut self, descriptor: QC::QueryDescriptor) -> bool {
        self.set.insert(descriptor)
    }

    /// Iterate over all queries in the set, in the order of their
    /// first insertion.
    pub fn iter(&self) -> impl Iterator<Item = &QC::QueryDescriptor> {
        self.set.iter()
    }
}

#[derive(Clone, Debug)]
crate struct StampedValue<V> {
    crate value: V,
    crate changed_at: Revision,
}
