use crate::class_table;
use crate::compiler::{CompilerQueryContext, Interner};
use salsa::query_context_storage;

/// Our "query context" is the context type that we will be threading
/// through our application (though 99% of the application only
/// interacts with it through traits and never knows its real name).
///
/// Query contexts can contain whatever you want them to, but salsa
/// requires two things:
///
/// - a salsa runtime (the `runtime` field, here)
/// - query storage (declared using the `query_context_storage` macro below)
#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::runtime::Runtime<QueryContextImpl>,
    storage: QueryContextImplStorage,
    interner: Interner,
}

/// This impl tells salsa where to find the runtime and storage in
/// your query context.
impl salsa::QueryContext for QueryContextImpl {
    fn salsa_storage(&self) -> &QueryContextImplStorage {
        &self.storage
    }

    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}

/// Declares the "query storage" for your context. Here, you list out
/// all of the query traits from your application that you wish to
/// provide storage for. This macro will generate the appropriate
/// storage and also generate impls for those traits, so that you
/// `QueryContextImpl` type implements them.
query_context_storage! {
    pub struct QueryContextImplStorage for QueryContextImpl {
        impl class_table::ClassTableQueryContext {
            fn all_classes() for class_table::AllClasses;
            fn all_fields() for class_table::AllFields;
            fn fields() for class_table::Fields;
        }
    }
}

/// In addition to the "query provider" traits, you may have other
/// trait requirements that your application needs -- you can
/// implement those yourself (in this case, an `interner`).
impl CompilerQueryContext for QueryContextImpl {
    fn interner(&self) -> &Interner {
        &self.interner
    }
}
