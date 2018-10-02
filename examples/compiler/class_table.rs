use crate::compiler;
use salsa::{query_definition, query_prototype};
use std::sync::Arc;

query_prototype! {
    pub trait ClassTableQueryContext: compiler::CompilerQueryContext {
        /// Get the fields.
        fn fields() for Fields;

        /// Get the list of all classes
        fn all_classes() for AllClasses;

        /// Get the list of all fields
        fn all_fields() for AllFields;
    }
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct DefId(usize);

query_definition! {
    pub AllClasses(_: &impl ClassTableQueryContext, (): ()) -> Arc<Vec<DefId>> {
        Arc::new(vec![DefId(0), DefId(10)]) // dummy impl
    }
}

query_definition! {
    pub Fields(_: &impl ClassTableQueryContext, class: DefId) -> Arc<Vec<DefId>> {
        Arc::new(vec![DefId(class.0 + 1), DefId(class.0 + 2)]) // dummy impl
    }
}

query_definition! {
    pub AllFields(query: &impl ClassTableQueryContext, (): ()) -> Arc<Vec<DefId>> {
        Arc::new(
            query.all_classes()
                .get(())
                .iter()
                .cloned()
                .flat_map(|def_id| {
                    let fields = query.fields().get(def_id);
                    (0..fields.len()).map(move |i| fields[i])
                })
                .collect()
        )
    }
}
