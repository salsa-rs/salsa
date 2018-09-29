use crate::Query;
use crate::QueryContext;
use crate::CycleDetected;
use rustc_hash::FxHashSet;
use std::cell::RefCell;
use std::fmt::Write;
use std::sync::Arc;

pub struct Runtime<QC>
where
    QC: QueryContext,
{
    storage: Arc<QC::QueryContextStorage>,
    /// The "call stack" of currently executing queries
    execution_stack: RefCell<Vec<QC::QueryDescriptor>>,
    /// Same data as `execution_stack` above, but as a HashSet.
    /// We use this to detect cycles.
    in_progress: RefCell<FxHashSet<QC::QueryDescriptor>>,
}

impl<QC> Default for Runtime<QC>
where
    QC: QueryContext,
{
    fn default() -> Self {
        Runtime {
            storage: Arc::default(),
            execution_stack: RefCell::default(),
            in_progress: RefCell::default(),
        }
    }
}

impl<QC> Runtime<QC>
where
    QC: QueryContext,
{
    pub fn storage(&self) -> &QC::QueryContextStorage {
        &self.storage
    }

    crate fn execute_query_implementation<Q>(
        &self,
        query: &QC,
        descriptor: QC::QueryDescriptor,
        key: &Q::Key,
    ) -> Result<Q::Value, CycleDetected>
    where
        Q: Query<QC>,
    {
        if !self.in_progress.borrow_mut().insert(descriptor.clone()) {
            return Err(CycleDetected);
        }
        self.execution_stack.borrow_mut().push(descriptor);
        let value = Q::execute(query, key.clone());
        let descriptor = self.execution_stack.borrow_mut().pop()
            .unwrap();
        self.in_progress.borrow_mut().remove(&descriptor);
        Ok(value)
    }

    /// Obviously, this should be user configurable at some point.
    crate fn report_unexpected_cycle(&self, descriptor: QC::QueryDescriptor) -> ! {
        let execution_stack = self.execution_stack.borrow();
        let start_index = (0..execution_stack.len())
            .rev()
            .filter(|&i| execution_stack[i] == descriptor)
            .next()
            .unwrap();

        let mut message = format!("Internal error, cycle detected:\n");
        for descriptor in &execution_stack[start_index..] {
            writeln!(message, "- {:?}\n", descriptor).unwrap();
        }
        panic!(message)
    }
}
