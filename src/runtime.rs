use crate::Query;
use crate::QueryContext;
use std::cell::RefCell;
use std::fmt::Write;
use std::sync::Arc;
use std::hash::{Hash, Hasher};

pub struct Runtime<QC>
where
    QC: QueryContext,
{
    storage: Arc<QC::QueryContextStorage>,
    execution_stack: RefCell<Vec<Frame<QC::QueryDescriptor>>>,
}

impl<QC> Default for Runtime<QC>
where
    QC: QueryContext,
{
    fn default() -> Self {
        Runtime {
            storage: Arc::default(),
            execution_stack: RefCell::default(),
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
    ) -> Q::Value
    where
        Q: Query<QC>,
    {
        let frame = Frame::new(descriptor);
        self.execution_stack.borrow_mut().push(frame);
        let value = Q::execute(query, key.clone());
        let frame = self.execution_stack.borrow_mut().pop()
            .unwrap();
        if let Some(caller_frame) = self.execution_stack.borrow_mut().last_mut() {
            let fingerprint = Fingerprint::new(&value);
            // FIXME: this won't record dependency information if the
            // query result is fetched from cache.
            caller_frame.dependencies.push((frame.query, fingerprint))
        }
        value
    }

    /// Obviously, this should be user configurable at some point.
    crate fn report_unexpected_cycle(&self, descriptor: QC::QueryDescriptor) -> ! {
        let execution_stack = self.execution_stack.borrow();
        let start_index = (0..execution_stack.len())
            .rev()
            .filter(|&i| execution_stack[i].query == descriptor)
            .next()
            .unwrap();

        let mut message = format!("Internal error, cycle detected:\n");
        for descriptor in &execution_stack[start_index..] {
            writeln!(message, "- {:?}\n", descriptor).unwrap();
        }
        panic!(message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint(u64);

type StableHasher = ::std::collections::hash_map::DefaultHasher;

impl Fingerprint {
    fn new(value: &impl Hash) -> Fingerprint {
        let mut hasher = StableHasher::new();
        value.hash(&mut hasher);
        let hash = hasher.finish();
        Fingerprint(hash)
    }
}

#[derive(Debug)]
struct Frame<QD> {
    query: QD,
    dependencies: Vec<(QD, Fingerprint)>,
}

impl<QD> Frame<QD> {
    fn new(query: QD) -> Frame<QD> {
        Frame {
            query,
            dependencies: Vec::new(),
        }
    }
}

