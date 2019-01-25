use crate::compiler;
use std::sync::Arc;

#[salsa::query_group(ClassTable)]
pub trait ClassTableDatabase: compiler::CompilerDatabase {
    /// Get the fields.
    fn fields(&self, class: DefId) -> Arc<Vec<DefId>>;

    /// Get the list of all classes
    fn all_classes(&self) -> Arc<Vec<DefId>>;

    /// Get the list of all fields
    fn all_fields(&self) -> Arc<Vec<DefId>>;
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct DefId(usize);

fn all_classes(_: &impl ClassTableDatabase) -> Arc<Vec<DefId>> {
    Arc::new(vec![DefId(0), DefId(10)]) // dummy impl
}

fn fields(_: &impl ClassTableDatabase, class: DefId) -> Arc<Vec<DefId>> {
    Arc::new(vec![DefId(class.0 + 1), DefId(class.0 + 2)]) // dummy impl
}

fn all_fields(db: &impl ClassTableDatabase) -> Arc<Vec<DefId>> {
    Arc::new(
        db.all_classes()
            .iter()
            .cloned()
            .flat_map(|def_id| {
                let fields = db.fields(def_id);
                (0..fields.len()).map(move |i| fields[i])
            })
            .collect(),
    )
}
