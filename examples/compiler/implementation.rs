use crate::class_table;
use crate::compiler::{CompilerQueryContext, Interner};
use salsa::query_context_storage;

/// Our "query context" is the context type that we will be threading
/// through our application (though 99% of the application only
/// interacts with it through traits and never knows its real name).
///
/// Query contexts can contain whatever you want them to, but salsa
/// requires you to add a `salsa::runtime::Runtime` member. Note
/// though: you should be very careful if adding shared, mutable state
/// to your context (e.g., a shared counter or some such thing). If
/// mutations to that shared state affect the results of your queries,
/// that's going to mess up the incremental results.
#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::runtime::Runtime<QueryContextImpl>,

    /// An interner is an example of shared mutable state that would
    /// be ok: although the interner allocates internally when you
    /// intern something new, this never affects any previously
    /// interned values, so it's not going to affect query results.
    interner: Interner,
}

/// This impl tells salsa where to find the salsa runtime.
impl salsa::QueryContext for QueryContextImpl {
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
