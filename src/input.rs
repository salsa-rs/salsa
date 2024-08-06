use std::{
    any::Any,
    fmt,
    ops::DerefMut,
    sync::atomic::{AtomicU32, Ordering},
};

pub mod input_field;
pub mod setter;
mod struct_map;

use input_field::FieldIngredientImpl;
use struct_map::StructMap;

use crate::{
    cycle::CycleRecoveryStrategy,
    id::{AsId, FromId},
    ingredient::{fmt_index, Ingredient},
    key::{DatabaseKeyIndex, DependencyIndex},
    plumbing::{Jar, Stamp},
    zalsa::IngredientIndex,
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
        struct_index: crate::zalsa::IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        let struct_ingredient: IngredientImpl<C> = IngredientImpl::new(struct_index);
        let struct_map = struct_ingredient.struct_map.clone();

        std::iter::once(Box::new(struct_ingredient) as _)
            .chain((0..C::FIELD_DEBUG_NAMES.len()).map(|field_index| {
                Box::new(FieldIngredientImpl::new(
                    struct_index,
                    field_index,
                    struct_map.clone(),
                )) as _
            }))
            .collect()
    }
}

pub struct IngredientImpl<C: Configuration> {
    ingredient_index: IngredientIndex,
    counter: AtomicU32,
    struct_map: StructMap<C>,
    _phantom: std::marker::PhantomData<C::Struct>,
}

impl<C: Configuration> IngredientImpl<C> {
    pub fn new(index: IngredientIndex) -> Self {
        Self {
            ingredient_index: index,
            counter: Default::default(),
            struct_map: StructMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn database_key_index(&self, id: C::Struct) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.ingredient_index,
            key_index: id.as_id(),
        }
    }

    pub fn new_input(&self, fields: C::Fields, stamps: C::Stamps) -> C::Struct {
        // If declared as a singleton, only allow a single instance
        if C::IS_SINGLETON && self.counter.load(Ordering::Relaxed) >= 1 {
            panic!("singleton struct may not be duplicated");
        }

        let next_id = Id::from_u32(self.counter.fetch_add(1, Ordering::Relaxed));
        let value = Value {
            id: next_id,
            fields,
            stamps,
        };
        self.struct_map.insert(value)
    }

    /// Change the value of the field `field_index` to a new value.
    ///
    /// # Parameters
    ///
    /// * `runtime`, the salsa runtiem
    /// * `id`, id of the input struct
    /// * `field_index`, index of the field that will be changed
    /// * `durability`, durability of the new value
    /// * `setter`, function that modifies the fields tuple; should only modify the element for `field_index`
    pub fn set_field<R>(
        &mut self,
        runtime: &mut Runtime,
        id: C::Struct,
        field_index: usize,
        durability: Durability,
        setter: impl FnOnce(&mut C::Fields) -> R,
    ) -> R {
        let id: Id = id.as_id();
        let mut r = self.struct_map.update(id);
        let stamp = &mut r.stamps[field_index];

        if stamp.durability != Durability::LOW {
            runtime.report_tracked_write(stamp.durability);
        }

        stamp.durability = durability;
        stamp.changed_at = runtime.current_revision();
        setter(&mut r.fields)
    }

    /// Get the singleton input previously created (if any).
    pub fn get_singleton_input(&self) -> Option<C::Struct> {
        assert!(
            C::IS_SINGLETON,
            "get_singleton_input invoked on a non-singleton"
        );
        (self.counter.load(Ordering::Relaxed) > 0).then(|| C::Struct::from_id(Id::from_u32(0)))
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
        let zalsa_local = db.zalsa_local();
        let field_ingredient_index = self.ingredient_index.successor(field_index);
        let id = id.as_id();
        let value = self.struct_map.get(id);
        let stamp = &value.stamps[field_index];
        zalsa_local.report_tracked_read(
            DependencyIndex {
                ingredient_index: field_ingredient_index,
                key_index: Some(id),
            },
            stamp.durability,
            stamp.changed_at,
        );
        &value.fields
    }

    /// Peek at the field values without recording any read dependency.
    /// Used for debug printouts.
    pub fn leak_fields(&self, id: C::Struct) -> &C::Fields {
        let id = id.as_id();
        let value = self.struct_map.get(id);
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

    fn origin(&self, _key_index: Id) -> Option<QueryOrigin> {
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

    fn salsa_struct_deleted(&self, _db: &dyn Database, _id: Id) {
        panic!(
            "unexpected call: input ingredients do not register for salsa struct deletion events"
        );
    }

    fn fmt_index(&self, index: Option<Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
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
    /// The id of this struct in the ingredient.
    id: Id,

    /// Fields of this input struct. They can change across revisions,
    /// but they do not change within a particular revision.
    fields: C::Fields,

    /// The revision and durability information for each field: when did this field last change.
    stamps: C::Stamps,
}

pub trait HasBuilder {
    type Builder;
}
