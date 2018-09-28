use crate::class_table;
use crate::compiler::{CompilerQueryContext, Interner};
use salsa::dyn_descriptor::DynDescriptor;
use salsa::query_context_storage;
use salsa::BaseQueryContext;
use salsa::Query;
use salsa::QueryTable;
use std::cell::RefCell;
use std::fmt::Write;

#[derive(Default)]
pub struct QueryContextImpl {
    storage: QueryContextImplStorage,
    interner: Interner,
    execution_stack: RefCell<Vec<DynDescriptor>>,
}

query_context_storage! {
    struct QueryContextImplStorage for storage in QueryContextImpl {
        impl class_table::ClassTableQueryContext {
            fn all_classes() for class_table::AllClasses;
            fn all_fields() for class_table::AllFields;
            fn fields() for class_table::Fields;
        }
    }
}

impl CompilerQueryContext for QueryContextImpl {
    fn interner(&self) -> &Interner {
        &self.interner
    }
}

impl BaseQueryContext for QueryContextImpl {
    type QueryDescriptor = DynDescriptor;

    fn execute_query_implementation<Q>(
        &self,
        descriptor: Self::QueryDescriptor,
        key: &Q::Key,
    ) -> Q::Value
    where
        Q: Query<Self>,
    {
        self.execution_stack.borrow_mut().push(descriptor);
        let value = Q::execute(self, key.clone());
        self.execution_stack.borrow_mut().pop();
        value
    }

    fn report_unexpected_cycle(&self, descriptor: Self::QueryDescriptor) -> ! {
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
