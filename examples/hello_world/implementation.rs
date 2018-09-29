use crate::class_table;
use crate::compiler::{CompilerQueryContext, Interner};
use salsa::query_context_storage;

#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::Runtime<QueryContextImpl>,
    storage: QueryContextImplStorage,
    interner: Interner,
}

// This is an example of how you "link up" all the queries in your
// application.
query_context_storage! {
    struct QueryContextImplStorage for storage in QueryContextImpl {
        impl class_table::ClassTableQueryContext {
            fn all_classes() for class_table::AllClasses;
            fn all_fields() for class_table::AllFields;
            fn fields() for class_table::Fields;
        }
    }
}

// This is an example of how you provide custom bits of stuff that
// your queries may need; in this case, an `Interner` value.
impl CompilerQueryContext for QueryContextImpl {
    fn interner(&self) -> &Interner {
        &self.interner
    }
}

// FIXME: This code... probably should not live here. But maybe we
// just want to provide some helpers or something? I do suspect I want
// people to be able to customize this.
//
// Seems like a classic case where specialization could be useful to
// permit behavior refinement.

impl salsa::QueryContext for QueryContextImpl {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}
