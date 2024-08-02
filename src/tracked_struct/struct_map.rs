use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use crossbeam::queue::SegQueue;
use dashmap::mapref::one::RefMut;

use crate::{alloc::Alloc, hash::FxDashMap, Id, Revision, Runtime};

use super::{Configuration, KeyStruct, Value};

pub(crate) struct StructMap<C>
where
    C: Configuration,
{
    map: Arc<FxDashMap<Id, Alloc<Value<C>>>>,

    /// When specific entities are deleted, their data is added
    /// to this vector rather than being immediately freed. This is because we may` have
    /// references to that data floating about that are tied to the lifetime of some
    /// `&db` reference. This queue itself is not freed until we have an `&mut db` reference,
    /// guaranteeing that there are no more references to it.
    deleted_entries: SegQueue<Alloc<Value<C>>>,
}

pub(crate) struct StructMapView<C>
where
    C: Configuration,
{
    map: Arc<FxDashMap<Id, Alloc<Value<C>>>>,
}

impl<C: Configuration> Clone for StructMapView<C> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

/// Return value for [`StructMap`][]'s `update` method.
pub(crate) enum Update<'db, C>
where
    C: Configuration,
{
    /// Indicates that the given struct has not yet been verified in this revision.
    /// The [`UpdateRef`][] gives mutable access to the field contents so that
    /// its fields can be compared and updated.
    Outdated(UpdateRef<'db, C>),

    /// Indicates that we have already verified that all the inputs accessed prior
    /// to this struct creation were up-to-date, and therefore the field contents
    /// ought not to have changed (barring user error). Returns a shared reference
    /// because caller cannot safely modify fields at this point.
    Current(C::Struct<'db>),
}

impl<C> StructMap<C>
where
    C: Configuration,
{
    pub fn new() -> Self {
        Self {
            map: Arc::new(FxDashMap::default()),
            deleted_entries: SegQueue::new(),
        }
    }

    /// Get a secondary view onto this struct-map that can be used to fetch entries.
    pub fn view(&self) -> StructMapView<C> {
        StructMapView {
            map: self.map.clone(),
        }
    }

    /// Insert the given tracked struct value into the map.
    ///
    /// # Panics
    ///
    /// * If value with same `value.id` is already present in the map.
    /// * If value not created in current revision.
    pub fn insert<'db>(&'db self, current_revision: Revision, value: Value<C>) -> C::Struct<'db> {
        assert_eq!(value.created_at, current_revision);

        let id = value.id;
        let boxed_value = Alloc::new(value);
        let pointer = boxed_value.as_raw();

        let old_value = self.map.insert(id, boxed_value);
        assert!(old_value.is_none()); // ...strictly speaking we probably need to abort here

        // Unsafety clause:
        //
        // * The box is owned by self and, although the box has been moved,
        //   the pointer is to the contents of the box, which have a stable
        //   address.
        // * Values are only removed or altered when we have `&mut self`.
        unsafe { C::struct_from_raw(pointer) }
    }

    /// Mark the given tracked struct as valid in the current revision.
    pub fn validate<'db>(&'db self, runtime: &'db Runtime, id: Id) {
        Self::validate_in_map(&self.map, runtime, id)
    }

    /// Internal helper to provide shared functionality for [`StructMapView`].
    fn validate_in_map<'db>(
        map: &'db FxDashMap<Id, Alloc<Value<C>>>,
        runtime: &'db Runtime,
        id: Id,
    ) {
        let mut data = map.get_mut(&id).unwrap();

        // UNSAFE: We never permit `&`-access in the current revision until data.created_at
        // has been updated to the current revision (which we check below).
        let data = unsafe { data.as_mut() };

        // Never update a struct twice in the same revision.
        let current_revision = runtime.current_revision();
        assert!(data.created_at < current_revision);
        data.created_at = current_revision;
    }

    /// Get mutable access to the data for `id` -- this holds a write lock for the duration
    /// of the returned value.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value is already updated in this revision.
    pub fn update<'db>(&'db self, current_revision: Revision, id: Id) -> Update<'db, C> {
        let mut data = self.map.get_mut(&id).unwrap();

        // UNSAFE: We never permit `&`-access in the current revision until data.created_at
        // has been updated to the current revision (which we check below).
        let data_ref = unsafe { data.as_mut() };

        // Subtle: it's possible that this struct was already validated
        // in this revision. What can happen (e.g., in the test
        // `test_run_5_then_20` in `specify_tracked_fn_in_rev_1_but_not_2.rs`)
        // is that
        //
        // * Revision 1:
        //   * Tracked function F creates tracked struct S
        //   * F reads input I
        //
        // In Revision 2, I is changed, and F is re-executed.
        // We try to validate F's inputs/outputs, which is the list [output: S, input: I].
        // As no inputs have changed by the time we reach S, we mark it as verified.
        // But then input I is seen to hvae changed, and so we re-execute F.
        // Note that we *know* that S will have the same value (barring program bugs).
        //
        // Further complicating things: it is possible that F calls F2
        // and gives it (e.g.) S as one of its arguments. Validating F2 may cause F2 to
        // re-execute which means that it may indeed have read from S's fields
        // during the current revision and thus obtained an `&` reference to those fields
        // that is still live.
        //
        // For this reason, we just return `None` in this case, ensuring that the calling
        // code cannot violate that `&`-reference.
        if data_ref.created_at == current_revision {
            drop(data);
            return Update::Current(Self::get_from_map(&self.map, current_revision, id));
        }

        data_ref.created_at = current_revision;
        Update::Outdated(UpdateRef { guard: data })
    }

    /// Lookup an existing tracked struct from the map.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value has not been updated in this revision.
    pub fn get<'db>(&'db self, current_revision: Revision, id: Id) -> C::Struct<'db> {
        Self::get_from_map(&self.map, current_revision, id)
    }

    /// Helper function, provides shared functionality for [`StructMapView`][]
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value has not been updated in this revision.
    fn get_from_map<'db>(
        map: &'db FxDashMap<Id, Alloc<Value<C>>>,
        current_revision: Revision,
        id: Id,
    ) -> C::Struct<'db> {
        let data = map.get(&id).unwrap();

        // UNSAFE: We permit `&`-access in the current revision once data.created_at
        // has been updated to the current revision (which we check below).
        let data_ref: &Value<C> = unsafe { data.as_ref() };

        // Before we drop the lock, check that the value has
        // been updated in this revision. This is what allows us to return a Struct
        let created_at = data_ref.created_at;
        assert!(
            created_at == current_revision,
            "access to tracked struct from previous revision"
        );

        // Unsafety clause:
        //
        // * Value will not be updated again in this revision,
        //   and revision will not change so long as runtime is shared
        // * We only remove values from the map when we have `&mut self`
        unsafe { C::struct_from_raw(data.as_raw()) }
    }

    /// Lookup an existing tracked struct from the map, maybe validating it to current revision.
    ///
    /// Validates to current revision if the struct was last created/validated in a revision that
    /// is still current for the struct's durability. That is, if the struct is HIGH durability
    /// (created by a HIGH durability query) and was created in R2, and we are now at R3 but no
    /// HIGH durability input has changed since R2, the struct is still valid and we can validate
    /// it to R3.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value has not been updated in the last-changed revision for its durability.
    fn get_and_validate_last_changed<'db>(
        map: &'db FxDashMap<Id, Alloc<Value<C>>>,
        runtime: &'db Runtime,
        id: Id,
    ) -> C::Struct<'db> {
        let data = map.get(&id).unwrap();

        // UNSAFE: We permit `&`-access in the current revision once data.created_at
        // has been updated to the current revision (which we ensure below).
        let data_ref: &Value<C> = unsafe { data.as_ref() };

        // Before we drop the lock, check that the value has been updated in the most recent
        // version in which the query that created it could have changed (based on durability).
        let created_at = data_ref.created_at;
        let last_changed = runtime.last_changed_revision(data_ref.durability);
        assert!(
            created_at >= last_changed,
            "access to tracked struct from obsolete revision"
        );

        // Unsafety clause:
        //
        // * Value will not be updated again in this revision,
        //   and revision will not change so long as runtime is shared
        // * We only remove values from the map when we have `&mut self`
        let ret = unsafe { C::struct_from_raw(data.as_raw()) };

        drop(data);

        // Validate in current revision, if necessary.
        if last_changed < runtime.current_revision() {
            Self::validate_in_map(map, runtime, id);
        }

        ret
    }

    /// Remove the entry for `id` from the map.
    ///
    /// NB. the data won't actually be freed until `drop_deleted_entries` is called.
    pub fn delete(&self, id: Id) -> Option<KeyStruct> {
        if let Some((_, data)) = self.map.remove(&id) {
            // UNSAFE: The `key` field is immutable once `ValueStruct` is created.
            let key = unsafe { data.as_ref() }.key;
            self.deleted_entries.push(data);
            Some(key)
        } else {
            None
        }
    }

    /// Drop all entries deleted until now.
    pub fn drop_deleted_entries(&mut self) {
        std::mem::take(&mut self.deleted_entries);
    }
}

impl<C> StructMapView<C>
where
    C: Configuration,
{
    /// Get a pointer to the data for the given `id`.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value has not been updated in this revision.
    pub fn get<'db>(&'db self, current_revision: Revision, id: Id) -> C::Struct<'db> {
        StructMap::get_from_map(&self.map, current_revision, id)
    }

    /// Lookup an existing tracked struct from the map, maybe validating it to current revision.
    ///
    /// See [`StructMap::get_and_validate_last_changed`].
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value has not been updated in the last-changed revision for its durability.
    pub(super) fn get_and_validate_last_changed<'db>(
        &'db self,
        runtime: &'db Runtime,
        id: Id,
    ) -> C::Struct<'db> {
        StructMap::get_and_validate_last_changed(&self.map, runtime, id)
    }
}

/// A mutable reference to the data for a single struct.
/// Can be "frozen" to yield an `&` that will remain valid
/// until the end of the revision.
pub(crate) struct UpdateRef<'db, C>
where
    C: Configuration,
{
    guard: RefMut<'db, Id, Alloc<Value<C>>>,
}

impl<'db, C> UpdateRef<'db, C>
where
    C: Configuration,
{
    /// Finalize this update, freezing the value for the rest of the revision.
    pub fn freeze(self) -> C::Struct<'db> {
        // Unsafety clause:
        //
        // see `get` above
        let data = self.guard.as_raw();
        unsafe { C::struct_from_raw(data) }
    }
}

impl<C> Deref for UpdateRef<'_, C>
where
    C: Configuration,
{
    type Target = Value<C>;

    fn deref(&self) -> &Self::Target {
        unsafe { self.guard.as_ref() }
    }
}

impl<C> DerefMut for UpdateRef<'_, C>
where
    C: Configuration,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.guard.as_mut() }
    }
}
