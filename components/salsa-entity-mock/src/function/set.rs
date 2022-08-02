use crossbeam::atomic::AtomicCell;

use crate::{
    entity::EntityInDb,
    key::DependencyIndex,
    runtime::local_state::{QueryInputs, QueryRevisions},
    Database,
};

use super::{memo::Memo, Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub fn set<'db>(&self, db: &'db DynDb<'db, C>, key: C::Key, value: C::Value)
    where
        C::Key: EntityInDb<DynDb<'db, C>>,
    {
        let runtime = db.salsa_runtime();

        let (active_query, current_deps) = match runtime.active_query() {
            Some(v) => v,
            None => panic!("can only use `set` with an active query"),
        };

        let entity_index = key.database_key_index(db);
        if !runtime.was_entity_created(entity_index) {
            panic!("can only use `set` on entities created during current query");
        }

        let revision = runtime.current_revision();
        let mut revisions = QueryRevisions {
            changed_at: current_deps.changed_at,
            durability: current_deps.durability,
            inputs: QueryInputs {
                untracked: false,
                tracked: std::iter::once(DependencyIndex::from(active_query)).collect(),
            },
        };

        if let Some(old_memo) = self.memo_map.get(key) {
            self.backdate_if_appropriate(&old_memo, &mut revisions, &value);
        }

        let memo = Memo {
            value: Some(value),
            verified_at: AtomicCell::new(revision),
            revisions,
        };

        self.insert_memo(key, memo);
    }
}
