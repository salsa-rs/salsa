// ANCHOR: db_struct
#[salsa::db(crate::Jar)]
pub(crate) struct Database {
    storage: salsa::Storage<Self>,
}
// ANCHOR_END: db_struct

// ANCHOR: default_impl
impl Default for Database {
    fn default() -> Self {
        Self {
            storage: Default::default(),
        }
    }
}
// ANCHOR_END: default_impl

// ANCHOR: db_impl
impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
}
// ANCHOR_END: db_impl

// ANCHOR: par_db_impl
impl salsa::ParallelDatabase for Database {
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Database {
            storage: self.storage.snapshot(),
        })
    }
}
// ANCHOR_END: par_db_impl
