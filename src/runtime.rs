use crate::Database;
use crate::Query;
use crate::QueryFunction;
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
pub struct Runtime<DB: Database> {
    shared_state: Arc<SharedState<DB>>,
    local_state: RefCell<LocalState<DB>>,
}

impl<DB> Default for Runtime<DB>
where
    DB: Database,
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

impl<DB> Runtime<DB>
where
    DB: Database,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn storage(&self) -> &DB::DatabaseStorage {
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
        if !self.local_state.borrow().query_stack.is_empty() {
            panic!("increment_revision invoked during a query computation");
        }

        let result = Revision {
            generation: 1 + self.shared_state.revision.fetch_add(1, Ordering::SeqCst),
        };

        debug!("increment_revision: incremented to {:?}", result);

        result
    }

    crate fn execute_query_implementation<Q>(
        &self,
        db: &DB,
        descriptor: &DB::QueryDescriptor,
        key: &Q::Key,
    ) -> (StampedValue<Q::Value>, QueryDescriptorSet<DB>)
    where
        Q: QueryFunction<DB>,
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
        let value = Q::execute(db, key.clone());

        // Extract accumulated inputs.
        let ActiveQuery {
            subqueries,
            changed_at,
            ..
        } = {
            let mut local_state = self.local_state.borrow_mut();

            // Sanity check: pushes and pops should be balanced.
            assert_eq!(local_state.query_stack.len(), push_len);

            local_state.query_stack.pop().unwrap()
        };

        (StampedValue { value, changed_at }, subqueries)
    }

    /// Reports that the currently active query read the result from
    /// another query.
    ///
    /// # Parameters
    ///
    /// - `descriptor`: the query whose result was read
    /// - `changed_revision`: the last revision in which the result of that
    ///   query had changed
    crate fn report_query_read(&self, descriptor: &DB::QueryDescriptor, changed_at: ChangedAt) {
        if let Some(top_query) = self.local_state.borrow_mut().query_stack.last_mut() {
            top_query.add_read(descriptor, changed_at);
        }
    }

    /// Obviously, this should be user configurable at some point.
    crate fn report_unexpected_cycle(&self, descriptor: DB::QueryDescriptor) -> ! {
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
struct SharedState<DB: Database> {
    storage: DB::DatabaseStorage,
    revision: AtomicU64,
}

/// State that will be specific to a single execution threads (when we
/// support multiple threads)
struct LocalState<DB: Database> {
    query_stack: Vec<ActiveQuery<DB>>,
}

struct ActiveQuery<DB: Database> {
    /// What query is executing
    descriptor: DB::QueryDescriptor,

    /// Records the maximum revision where any subquery changed
    changed_at: ChangedAt,

    /// Each subquery
    subqueries: QueryDescriptorSet<DB>,
}

impl<DB: Database> ActiveQuery<DB> {
    fn new(descriptor: DB::QueryDescriptor) -> Self {
        ActiveQuery {
            descriptor,
            changed_at: ChangedAt::Revision(Revision::ZERO),
            subqueries: QueryDescriptorSet::new(),
        }
    }

    fn add_read(&mut self, subquery: &DB::QueryDescriptor, changed_at: ChangedAt) {
        self.subqueries.insert(subquery.clone());
        self.changed_at = self.changed_at.max(changed_at);
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

/// Records when a stamped value changed.
///
/// Note: the order of variants is significant. We sometimes use `max`
/// for example to find the "most recent revision" when something
/// changed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangedAt {
    Revision(Revision),
}

impl ChangedAt {
    /// True if this value has changed after `revision`.
    pub fn changed_since(self, revision: Revision) -> bool {
        match self {
            ChangedAt::Revision(r) => r > revision,
        }
    }
}

/// An insertion-order-preserving set of queries. Used to track the
/// inputs accessed during query execution.
crate struct QueryDescriptorSet<DB: Database> {
    set: FxIndexSet<DB::QueryDescriptor>,
}

impl<DB: Database> std::fmt::Debug for QueryDescriptorSet<DB> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.set, fmt)
    }
}

impl<DB: Database> QueryDescriptorSet<DB> {
    crate fn new() -> Self {
        QueryDescriptorSet {
            set: FxIndexSet::default(),
        }
    }

    /// Add `descriptor` to the set. Returns true if `descriptor` is
    /// newly added and false if `descriptor` was already a member.
    fn insert(&mut self, descriptor: DB::QueryDescriptor) -> bool {
        self.set.insert(descriptor)
    }

    /// Iterate over all queries in the set, in the order of their
    /// first insertion.
    pub fn iter(&self) -> impl Iterator<Item = &DB::QueryDescriptor> {
        self.set.iter()
    }
}

#[derive(Clone, Debug)]
crate struct StampedValue<V> {
    crate value: V,
    crate changed_at: ChangedAt,
}
