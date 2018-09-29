use crate::BaseQueryContext;
use crate::Query;
use std::cell::RefCell;
use std::fmt::Write;

pub struct Runtime<QC>
where
    QC: BaseQueryContext,
{
    execution_stack: RefCell<Vec<QC::QueryDescriptor>>,
}

impl<QC> Default for Runtime<QC>
where
    QC: BaseQueryContext,
{
    fn default() -> Self {
        Runtime {
            execution_stack: RefCell::default(),
        }
    }
}

impl<QC> Runtime<QC>
where
    QC: BaseQueryContext,
{
    crate fn execute_query_implementation<Q>(
        &self,
        query: &QC,
        descriptor: QC::QueryDescriptor,
        key: &Q::Key,
    ) -> Q::Value
    where
        Q: Query<QC>,
    {
        self.execution_stack.borrow_mut().push(descriptor);
        let value = Q::execute(query, key.clone());
        self.execution_stack.borrow_mut().pop();
        value
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
