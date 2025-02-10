use crate::{zalsa::transmute_data_ptr, Database};
use std::{
    any::{Any, TypeId},
    sync::Arc,
};

/// A `Views` struct is associated with some specific database type
/// (a `DatabaseImpl<U>` for some existential `U`). It contains functions
/// to downcast from that type to `dyn DbView` for various traits `DbView`.
/// None of these types are known at compilation time, they are all checked
/// dynamically through `TypeId` magic.
///
/// You can think of the struct as looking like:
///
/// ```rust,ignore
/// struct Views<ghost Db> {
///     source_type_id: TypeId,                       // `TypeId` for `Db`
///     view_casters: Arc<ConcurrentVec<exists<DbView> {
///         ViewCaster<Db, DbView>
///     }>>,
/// }
/// ```
#[derive(Clone)]
pub struct Views {
    source_type_id: TypeId,
    view_casters: Arc<boxcar::Vec<DynViewCaster>>,
}

/// A DynViewCaster contains a manual trait object that can cast from the
/// (ghost) `Db` type of `Views` to some (ghost) `DbView` type.
///
/// You can think of the struct as looking like:
///
/// ```rust,ignore
/// struct DynViewCaster<ghost Db, ghost DbView> {
///     target_type_id: TypeId,     // TypeId of DbView
///     type_name: &'static str,    // type name of DbView
///     view_caster: *mut (),       // a `Box<ViewCaster<Db, DbView>>`
///     cast: *const (),            // a `unsafe fn (&ViewCaster<Db, DbView>, &dyn Database) -> &DbView`
///     drop: *const (),            // the destructor for the box above
/// }
/// ```
///
/// The manual trait object and vtable allows for type erasure without
/// transmuting between fat pointers, whose layout is undefined.
struct DynViewCaster {
    /// The id of the target type `DbView` that we can cast to.
    target_type_id: TypeId,

    /// The name of the target type `DbView` that we can cast to.
    type_name: &'static str,

    /// A pointer to a `ViewCaster<Db, DbView>`.
    view_caster: *mut (),

    /// Type-erased `ViewCaster::<Db, DbView>::vtable_cast`.
    cast: *const (),

    /// Type-erased `ViewCaster::<Db, DbView>::drop`.
    drop: unsafe fn(*mut ()),
}

impl Drop for DynViewCaster {
    fn drop(&mut self) {
        // SAFETY: We own `self.caster` and are in the destructor.
        unsafe { (self.drop)(self.view_caster) };
    }
}

// SAFETY: These traits can be implemented normally as the raw pointers
// in `DynViewCaster` are only used for type-erasure.
unsafe impl Send for DynViewCaster {}
unsafe impl Sync for DynViewCaster {}

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        Self {
            source_type_id,
            view_casters: Arc::new(boxcar::Vec::new()),
        }
    }

    /// Add a new upcast from `Db` to `T`, given the upcasting function `func`.
    pub fn add<Db: Database, DbView: ?Sized + Any>(&self, func: fn(&Db) -> &DbView) {
        assert_eq!(self.source_type_id, TypeId::of::<Db>(), "dyn-upcasts");

        let target_type_id = TypeId::of::<DbView>();

        if self
            .view_casters
            .iter()
            .any(|(_, u)| u.target_type_id == target_type_id)
        {
            return;
        }

        let view_caster = Box::into_raw(Box::new(ViewCaster(func)));

        self.view_casters.push(DynViewCaster {
            target_type_id,
            type_name: std::any::type_name::<DbView>(),
            view_caster: view_caster.cast(),
            cast: ViewCaster::<Db, DbView>::erased_cast as _,
            drop: ViewCaster::<Db, DbView>::erased_drop,
        });
    }

    /// Convert one handle to a salsa database (including a `dyn Database`!) to another.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn try_view_as<'db, DbView: ?Sized + Any>(
        &self,
        db: &'db dyn Database,
    ) -> Option<&'db DbView> {
        let db_type_id = <dyn Database as Any>::type_id(db);
        assert_eq!(self.source_type_id, db_type_id, "database type mismatch");

        let view_type_id = TypeId::of::<DbView>();
        for (_idx, view) in self.view_casters.iter() {
            if view.target_type_id == view_type_id {
                // SAFETY: We verified that this is the view caster for the
                // `DbView` type by checking type IDs above.
                let view = unsafe {
                    let caster: unsafe fn(*const (), &dyn Database) -> &DbView =
                        std::mem::transmute(view.cast);
                    caster(view.view_caster, db)
                };

                return Some(view);
            }
        }

        None
    }
}

/// A generic downcaster for specific `Db` and `DbView` types.
struct ViewCaster<Db, DbView: ?Sized>(fn(&Db) -> &DbView);

impl<Db, DbView> ViewCaster<Db, DbView>
where
    Db: Database,
    DbView: ?Sized + Any,
{
    /// Obtain a reference of type `DbView` from a database.
    ///
    /// # Safety
    ///
    /// The input database must be of type `Db`.
    unsafe fn cast<'db>(&self, db: &'db dyn Database) -> &'db DbView {
        // This tests the safety requirement:
        debug_assert_eq!(db.type_id(), TypeId::of::<Db>());

        // SAFETY:
        //
        // Caller guarantees that the input is of type `Db`
        // (we test it in the debug-assertion above).
        let db = unsafe { transmute_data_ptr::<dyn Database, Db>(db) };
        (self.0)(db)
    }

    /// A type-erased version of `ViewCaster::<Db, DbView>::cast`.
    ///
    /// # Safety
    ///
    /// The underlying type of `caster` must be `ViewCaster::<Db, DbView>`.
    unsafe fn erased_cast(caster: *mut (), db: &dyn Database) -> &DbView {
        let caster = unsafe { &*caster.cast::<ViewCaster<Db, DbView>>() };
        caster.cast(db)
    }

    /// The destructor for `Box<ViewCaster<Db, DbView>>`.
    ///
    /// # Safety
    ///
    /// All the safety requirements of `Box::<ViewCaster<Db, DbView>>::from_raw` apply.
    unsafe fn erased_drop(caster: *mut ()) {
        let _: Box<ViewCaster<Db, DbView>> = unsafe { Box::from_raw(caster.cast()) };
    }
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Views")
            .field("view_casters", &self.view_casters)
            .finish()
    }
}

impl std::fmt::Debug for DynViewCaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynViewCaster")
            .field(&self.type_name)
            .finish()
    }
}
