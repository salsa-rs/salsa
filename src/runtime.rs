use crate::Query;
use crate::QueryContext;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Runtime<QC>
where
    QC: QueryContext,
{
    shared_state: Arc<SharedState<QC>>,
    local_state: RefCell<LocalState<QC>>,
}

/// State that will be common to all threads (when we support multiple threads)
struct SharedState<QC>
where
    QC: QueryContext,
{
    storage: QC::QueryContextStorage,
    revision: AtomicU64,
}

/// State that will be specific to a single execution threads (when we support multiple threads)
struct LocalState<QC>
where
    QC: QueryContext,
{
    query_stack: Vec<QC::QueryDescriptor>,
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
        self.local_state.borrow_mut().query_stack.push(descriptor);
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
            .filter(|&i| query_stack[i] == descriptor)
            .next()
            .unwrap();

        let mut message = format!("Internal error, cycle detected:\n");
        for descriptor in &query_stack[start_index..] {
            writeln!(message, "- {:?}\n", descriptor).unwrap();
        }
        panic!(message)
    }
}
