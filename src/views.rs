use std::any::{Any, TypeId};

use crate::Database;

/// A `Views` struct is associated with some specific database type
/// (a `DatabaseImpl<U>` for some existential `U`). It contains functions
/// to downcast from `dyn Database` to `dyn DbView` for various traits `DbView` via this specific
/// database type.
/// None of these types are known at compilation time, they are all checked
/// dynamically through `TypeId` magic.
pub struct Views {
    source_type_id: TypeId,
    view_casters: boxcar::Vec<DynViewCaster>,
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
    /// The id of the target type `dyn DbView` that we can cast to.
    target_type_id: TypeId,

    /// The name of the target type `dyn DbView` that we can cast to.
    type_name: &'static str,

    /// Type-erased `ViewCaster::<Db, DbView>::vtable_cast`.
    cast: ErasedDatabaseDownCaster,
}

type ErasedDatabaseDownCaster = unsafe fn(&dyn Database) -> *const ();
pub type DatabaseDownCaster<DbView> = unsafe fn(&dyn Database) -> &DbView;

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        let view_casters = boxcar::Vec::new();
        // special case the no-op transformation, that way we skip out on reconstructing the wide pointer
        view_casters.push(DynViewCaster {
            target_type_id: TypeId::of::<dyn Database>(),
            type_name: std::any::type_name::<dyn Database>(),
            cast: unsafe {
                std::mem::transmute::<DatabaseDownCaster<dyn Database>, ErasedDatabaseDownCaster>(
                    |db| db,
                )
            },
        });
        Self {
            source_type_id,
            view_casters,
        }
    }

    /// Add a new downcaster from `dyn Database` to `dyn DbView`.
    pub fn add<DbView: ?Sized + Any>(&self, func: DatabaseDownCaster<DbView>) {
        let target_type_id = TypeId::of::<DbView>();
        if self
            .view_casters
            .iter()
            .any(|(_, u)| u.target_type_id == target_type_id)
        {
            return;
        }
        self.view_casters.push(DynViewCaster {
            target_type_id,
            type_name: std::any::type_name::<DbView>(),
            cast: unsafe {
                std::mem::transmute::<DatabaseDownCaster<DbView>, ErasedDatabaseDownCaster>(func)
            },
        });
    }

    /// Retrieve an downcaster function from `dyn Database` to `dyn DbView`.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn downcaster_for<DbView: ?Sized + Any>(&self) -> DatabaseDownCaster<DbView> {
        let view_type_id = TypeId::of::<DbView>();
        for (_idx, view) in self.view_casters.iter() {
            if view.target_type_id == view_type_id {
                return unsafe {
                    std::mem::transmute::<ErasedDatabaseDownCaster, DatabaseDownCaster<DbView>>(
                        view.cast,
                    )
                };
            }
        }

        panic!(
            "No downcaster registered for type `{}` in `Views`",
            std::any::type_name::<DbView>(),
        );
    }

    pub fn assert_database(&self, db: &dyn Database) {
        assert_eq!(
            self.source_type_id,
            db.type_id(),
            "Database type does not match the expected type for this `Views` instance"
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

impl std::fmt::Debug for DynViewCaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynViewCaster")
            .field(&self.type_name)
            .finish()
    }
}
