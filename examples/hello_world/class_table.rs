use crate::compiler;
use salsa::{query_definition, query_prototype};
use std::sync::Arc;

pub trait ClassTableQueryContext: compiler::CompilerQueryContext {
    query_prototype!(
        /// Get the fields.
        fn fields() for Fields
    );

    query_prototype!(
        /// Get the list of all classes
        fn all_classes() for AllClasses
    );

    query_prototype!(
        /// Get the list of all fields
        fn all_fields() for AllFields
    );
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DefId;

query_definition! {
    pub AllClasses(_: &impl ClassTableQueryContext, (): ()) -> Arc<Vec<DefId>> {
        Arc::new(vec![]) // dummy impl
    }
}

query_definition! {
    pub Fields(_: &impl ClassTableQueryContext, _class: DefId) -> Arc<Vec<DefId>> {
        Arc::new(vec![]) // dummy impl
    }
}

query_definition! {
    pub AllFields(query: &impl ClassTableQueryContext, _class: DefId) -> Arc<Vec<DefId>> {
        Arc::new(
            query.all_classes()
                .of(())
                .iter()
                .cloned()
                .flat_map(|def_id| {
                    let fields = query.fields().of(def_id);
                    (0..fields.len()).map(move |i| fields[i].clone())
                })
                .collect()
        )
    }
}
