use std::{any::Any, fmt, ops::DerefMut};

pub mod input_field;
pub mod setter;

use crossbeam::atomic::AtomicCell;
use input_field::FieldIngredientImpl;
use parking_lot::Mutex;

use crate::{
    cycle::CycleRecoveryStrategy,
    id::{AsId, FromId},
    ingredient::{fmt_index, Ingredient},
    key::{DatabaseKeyIndex, DependencyIndex},
    plumbing::{Jar, JarAux, Stamp},
    table::{memo::MemoTable, sync::SyncTable, Slot, Table},
    zalsa::{IngredientIndex, Zalsa},
    zalsa_local::QueryOrigin,
    Database, Durability, Id, Revision, Runtime,
};

pub trait Configuration: Any {
    const DEBUG_NAME: &'static str;
    const FIELD_DEBUG_NAMES: &'static [&'static str];
    const IS_SINGLETON: bool;

    /// The input struct (which wraps an `Id`)
    type Struct: FromId + 'static + Send + Sync;

    /// A (possibly empty) tuple of the fields for this struct.
    type Fields: Send + Sync;

    /// A array of [`StampedValue<()>`](`StampedValue`) tuples, one per each of the value fields.
    type Stamps: Send + Sync + fmt::Debug + DerefMut<Target = [Stamp]>;
}

pub struct JarImpl<C: Configuration> {
    _phantom: std::marker::PhantomData<C>,
}

impl<C: Configuration> Default for JarImpl<C> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<C: Configuration> Jar for JarImpl<C> {
    fn create_ingredients(
        &self,
        _aux: &dyn JarAux,
        struct_index: crate::zalsa::IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        let struct_ingredient: IngredientImpl<C> = IngredientImpl::new(struct_index);

        std::iter::once(Box::new(struct_ingredient) as _)
            .chain((0..C::FIELD_DEBUG_NAMES.len()).map(|field_index| {
                Box::new(<FieldIngredientImpl<C>>::new(struct_index, field_index)) as _
            }))
            .collect()
    }
}

pub struct IngredientImpl<C: Configuration> {
    ingredient_index: IngredientIndex,
    singleton_index: AtomicCell<Option<Id>>,
    singleton_lock: Mutex<()>,
    _phantom: std::marker::PhantomData<C::Struct>,
}

impl<C: Configuration> IngredientImpl<C> {
    pub fn new(index: IngredientIndex) -> Self {
        Self {
            ingredient_index: index,
            singleton_index: AtomicCell::new(None),
            singleton_lock: Default::default(),
            _phantom: std::marker::PhantomData,
        }
    }

    fn data(zalsa: &Zalsa, id: Id) -> &Value<C> {
        zalsa.table().get(id)
    }

    fn data_raw(table: &Table, id: Id) -> *mut Value<C> {
        table.get_raw(id)
    }

    pub fn database_key_index(&self, id: C::Struct) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.ingredient_index,
            key_index: id.as_id(),
        }
    }

    pub fn new_input(&self, db: &dyn Database, fields: C::Fields, stamps: C::Stamps) -> C::Struct {
        let (zalsa, zalsa_local) = db.zalsas();

        // If declared as a singleton, only allow a single instance
        let guard = if C::IS_SINGLETON {
            let guard = self.singleton_lock.lock();
            if self.singleton_index.load().is_some() {
                panic!("singleton struct may not be duplicated");
            }
            Some(guard)
        } else {
            None
        };

        let id = zalsa_local.allocate(zalsa.table(), self.ingredient_index, || Value::<C> {
            fields,
            stamps,
            memos: Default::default(),
            syncs: Default::default(),
        });

        if C::IS_SINGLETON {
            self.singleton_index.store(Some(id));
            drop(guard);
        }

        FromId::from_id(id)
    }

    /// Change the value of the field `field_index` to a new value.
    ///
    /// # Parameters
    ///
    /// * `runtime`, the salsa runtiem
    /// * `id`, id of the input struct
    /// * `field_index`, index of the field that will be changed
    /// * `durability`, durability of the new value. If omitted, uses the durability of the previous value.
    /// * `setter`, function that modifies the fields tuple; should only modify the element for `field_index`
    pub fn set_field<R>(
        &mut self,
        runtime: &mut Runtime,
        id: C::Struct,
        field_index: usize,
        durability: Option<Durability>,
        setter: impl FnOnce(&mut C::Fields) -> R,
    ) -> R {
        let id: Id = id.as_id();
        let r = Self::data_raw(runtime.table(), id);

        // SAFETY: We hold `&mut` on the runtime so no `&`-references can be active.
        // Also, we don't access any other data from the table while `r` is active.
        let r = unsafe { &mut *r };

        let stamp = &mut r.stamps[field_index];

        if stamp.durability != Durability::LOW {
            runtime.report_tracked_write(stamp.durability);
        }

        stamp.durability = durability.unwrap_or(stamp.durability);
        stamp.changed_at = runtime.current_revision();
        setter(&mut r.fields)
    }

    /// Get the singleton input previously created (if any).
    pub fn get_singleton_input(&self) -> Option<C::Struct> {
        assert!(
            C::IS_SINGLETON,
            "get_singleton_input invoked on a non-singleton"
        );
        self.singleton_index.load().map(FromId::from_id)
    }

    /// Access field of an input.
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responible for selecting the appropriate element.
    pub fn field<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        id: C::Struct,
        field_index: usize,
    ) -> &'db C::Fields {
        let (zalsa, zalsa_local) = db.zalsas();
        let field_ingredient_index = self.ingredient_index.successor(field_index);
        let id = id.as_id();
        let value = Self::data(zalsa, id);
        let stamp = &value.stamps[field_index];
        zalsa_local.report_tracked_read(
            DependencyIndex {
                ingredient_index: field_ingredient_index,
                key_index: Some(id),
            },
            stamp.durability,
            stamp.changed_at,
            &Default::default(),
        );
        &value.fields
    }

    /// Peek at the field values without recording any read dependency.
    /// Used for debug printouts.
    pub fn leak_fields<'db>(&'db self, db: &'db dyn Database, id: C::Struct) -> &'db C::Fields {
        let zalsa = db.zalsa();
        let id = id.as_id();
        let value = Self::data(zalsa, id);
        &value.fields
    }
}

impl<C: Configuration> Ingredient for IngredientImpl<C> {
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        _input: Option<Id>,
        _revision: Revision,
    ) -> bool {
        // Input ingredients are just a counter, they store no data, they are immortal.
        // Their *fields* are stored in function ingredients elsewhere.
        false
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _db: &dyn Database, _key_index: Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        executor: DatabaseKeyIndex,
        output_key: Option<Id>,
    ) {
        unreachable!(
            "mark_validated_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, output_key
        );
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<Id>,
    ) {
        unreachable!(
            "remove_stale_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, stale_output_key
        );
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        panic!("unexpected call to `reset_for_new_revision`")
    }

    fn fmt_index(&self, index: Option<Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn accumulated<'db>(
        &'db self,
        _db: &'db dyn Database,
        _key_index: Id,
    ) -> Option<&'db crate::accumulator::accumulated_map::AccumulatedMap> {
        None
    }
}

impl<C: Configuration> std::fmt::Debug for IngredientImpl<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.ingredient_index)
            .finish()
    }
}

#[derive(Debug)]
pub struct Value<C>
where
    C: Configuration,
{
    /// Fields of this input struct. They can change across revisions,
    /// but they do not change within a particular revision.
    fields: C::Fields,

    /// The revision and durability information for each field: when did this field last change.
    stamps: C::Stamps,

    /// Memos
    memos: MemoTable,

    /// Syncs
    syncs: SyncTable,
}

pub trait HasBuilder {
    type Builder;
}

impl<C> Slot for Value<C>
where
    C: Configuration,
{
    unsafe fn memos(&self, _current_revision: Revision) -> &crate::table::memo::MemoTable {
        &self.memos
    }

    unsafe fn syncs(&self, _current_revision: Revision) -> &SyncTable {
        &self.syncs
    }
}
