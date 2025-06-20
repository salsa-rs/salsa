use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    mem,
    ptr::NonNull,
};

use crate::{database::RawDatabasePointer, Database};

/// A `Views` struct is associated with some specific database type
/// (a `DatabaseImpl<U>` for some existential `U`). It contains functions
/// to upcast to `dyn DbView` for various traits `DbView` via this specific
/// database type.
/// None of these types are known at compilation time, they are all checked
/// dynamically through `TypeId` magic.
pub struct Views {
    source_type_id: TypeId,
    view_casters: boxcar::Vec<ViewCaster>,
}

#[derive(Copy, Clone)]
struct ViewCaster {
    /// The id of the target type `dyn DbView` that we can cast to.
    target_type_id: TypeId,

    /// The name of the target type `dyn DbView` that we can cast to.
    type_name: &'static str,

    /// Type-erased function pointer that upcasts to `dyn DbView`.
    cast: ErasedDatabaseUpCasterSig,
}

impl ViewCaster {
    fn new<DbView: ?Sized + Any>(func: DatabaseUpCasterSigRaw<DbView>) -> ViewCaster {
        ViewCaster {
            target_type_id: TypeId::of::<DbView>(),
            type_name: std::any::type_name::<DbView>(),
            // SAFETY: We are type erasing for storage, taking care of unerasing before we call
            // the function pointer.
            cast: unsafe {
                mem::transmute::<DatabaseUpCasterSigRaw<DbView>, ErasedDatabaseUpCasterSig>(func)
            },
        }
    }
}

type ErasedDatabaseUpCasterSig = unsafe fn(RawDatabasePointer<'_>) -> NonNull<()>;
type DatabaseUpCasterSigRaw<DbView> =
    for<'db> unsafe fn(RawDatabasePointer<'db>) -> NonNull<DbView>;
type DatabaseUpCasterSig<DbView> = for<'db> unsafe fn(RawDatabasePointer<'db>) -> &'db DbView;
type DatabaseUpCasterSigMut<DbView> =
    for<'db> unsafe fn(RawDatabasePointer<'db>) -> &'db mut DbView;

#[repr(transparent)]
pub struct DatabaseUpCaster<DbView: ?Sized>(ViewCaster, PhantomData<fn() -> DbView>);

impl<DbView: ?Sized> Copy for DatabaseUpCaster<DbView> {}
impl<DbView: ?Sized> Clone for DatabaseUpCaster<DbView> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<DbView: ?Sized + Any> DatabaseUpCaster<DbView> {
    /// Upcast `db` to `DbView`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `db` is of the correct type.
    #[inline]
    pub unsafe fn upcast_unchecked<'db>(&self, db: RawDatabasePointer<'db>) -> &'db DbView {
        // SAFETY: The caller must ensure that `db` is of the correct type.
        unsafe {
            (mem::transmute::<ErasedDatabaseUpCasterSig, DatabaseUpCasterSig<DbView>>(self.0.cast))(
                db,
            )
        }
    }
    /// Upcast `db` to `DbView`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `db` is of the correct type.
    #[inline]
    pub unsafe fn upcast_mut_unchecked<'db>(&self, db: RawDatabasePointer<'db>) -> &'db mut DbView {
        // SAFETY: The caller must ensure that `db` is of the correct type.
        unsafe {
            (mem::transmute::<ErasedDatabaseUpCasterSig, DatabaseUpCasterSigMut<DbView>>(
                self.0.cast,
            ))(db)
        }
    }
}

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        let view_casters = boxcar::Vec::new();
        view_casters.push(ViewCaster::new::<dyn Database>(|db| db.ptr.cast::<Db>()));
        Self {
            source_type_id,
            view_casters,
        }
    }

    /// Add a new upcaster to `dyn DbView`.
    pub fn add<Concrete: 'static, DbView: ?Sized + Any>(
        &self,
        func: fn(&Concrete) -> &DbView,
    ) -> &DatabaseUpCaster<DbView> {
        assert_eq!(self.source_type_id, TypeId::of::<Concrete>());
        let target_type_id = TypeId::of::<DbView>();
        if let Some((_, caster)) = self
            .view_casters
            .iter()
            .find(|(_, u)| u.target_type_id == target_type_id)
        {
            // SAFETY: The type-erased function pointer is guaranteed to be valid for `DbView`
            return unsafe { &*(caster as *const ViewCaster as *const DatabaseUpCaster<DbView>) };
        }

        // SAFETY: We are type erasing the function pointer for storage, and we will unerase it
        // before we call it.
        let caster = ViewCaster::new::<DbView>(unsafe {
            mem::transmute::<fn(&Concrete) -> &DbView, DatabaseUpCasterSigRaw<DbView>>(func)
        });
        let idx = self.view_casters.push(caster);
        // SAFETY: The type-erased function pointer is guaranteed to be valid for `DbView`
        unsafe { &*(&raw const self.view_casters[idx]).cast::<DatabaseUpCaster<DbView>>() }
    }

    #[inline]
    pub fn base_database_upcaster(&self) -> &DatabaseUpCaster<dyn Database> {
        // SAFETY: The type-erased function pointer is guaranteed to be valid for `dyn Database`
        // since we created it with the same type.
        unsafe { &*((&raw const self.view_casters[0]).cast::<DatabaseUpCaster<dyn Database>>()) }
    }

    /// Retrieve an upcaster function to `dyn DbView`.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn upcaster_for<DbView: ?Sized + Any>(&self) -> &DatabaseUpCaster<DbView> {
        let view_type_id = TypeId::of::<DbView>();
        for (_, view) in self.view_casters.iter() {
            if view.target_type_id == view_type_id {
                // SAFETY: We are unerasing the type erased function pointer having made sure the
                // TypeId matches.
                return unsafe {
                    &*((view as *const ViewCaster).cast::<DatabaseUpCaster<DbView>>())
                };
            }
        }

        panic!(
            "No upcaster registered for type `{}` in `Views`",
            std::any::type_name::<DbView>(),
        );
    }
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Views")
            .field("view_casters", &self.view_casters)
            .finish()
    }
}

impl std::fmt::Debug for ViewCaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynViewCaster")
            .field(&self.type_name)
            .finish()
    }
}
