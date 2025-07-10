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
    view_casters: boxcar::Vec<ViewCaster>,
}

struct ViewCaster {
    /// The id of the target type `dyn DbView` that we can cast to.
    target_type_id: TypeId,

    /// The name of the target type `dyn DbView` that we can cast to.
    type_name: &'static str,

    /// Type-erased function pointer that downcasts from `dyn Database` to `dyn DbView`.
    cast: ErasedDatabaseDownCasterSig,
}

impl ViewCaster {
    fn new<DbView: ?Sized + Any>(func: unsafe fn(&dyn Database) -> &DbView) -> ViewCaster {
        ViewCaster {
            target_type_id: TypeId::of::<DbView>(),
            type_name: std::any::type_name::<DbView>(),
            // SAFETY: We are type erasing for storage, taking care of unerasing before we call
            // the function pointer.
            cast: unsafe {
                std::mem::transmute::<DatabaseDownCasterSig<DbView>, ErasedDatabaseDownCasterSig>(
                    func,
                )
            },
        }
    }
}

type ErasedDatabaseDownCasterSig = unsafe fn(&dyn Database) -> *const ();
type DatabaseDownCasterSig<DbView> = unsafe fn(&dyn Database) -> &DbView;

pub struct DatabaseDownCaster<DbView: ?Sized>(TypeId, DatabaseDownCasterSig<DbView>);

impl<DbView: ?Sized + Any> DatabaseDownCaster<DbView> {
    pub fn downcast<'db>(&self, db: &'db dyn Database) -> &'db DbView {
        assert_eq!(
            self.0,
            db.type_id(),
            "Database type does not match the expected type for this `Views` instance"
        );
        // SAFETY: We've asserted that the database is correct.
        unsafe { (self.1)(db) }
    }

    /// Downcast `db` to `DbView`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `db` is of the correct type.
    pub unsafe fn downcast_unchecked<'db>(&self, db: &'db dyn Database) -> &'db DbView {
        // SAFETY: The caller must ensure that `db` is of the correct type.
        unsafe { (self.1)(db) }
    }
}

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        let view_casters = boxcar::Vec::new();
        // special case the no-op transformation, that way we skip out on reconstructing the wide pointer
        view_casters.push(ViewCaster::new::<dyn Database>(|db| db));
        Self {
            source_type_id,
            view_casters,
        }
    }

    /// Add a new downcaster from `dyn Database` to `dyn DbView`.
    pub fn add<DbView: ?Sized + Any>(
        &self,
        func: DatabaseDownCasterSig<DbView>,
    ) -> DatabaseDownCaster<DbView> {
        if let Some(view) = self.try_downcaster_for() {
            return view;
        }

        self.view_casters.push(ViewCaster::new::<DbView>(func));
        DatabaseDownCaster(self.source_type_id, func)
    }

    /// Retrieve an downcaster function from `dyn Database` to `dyn DbView`.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn downcaster_for<DbView: ?Sized + Any>(&self) -> DatabaseDownCaster<DbView> {
        self.try_downcaster_for().unwrap_or_else(|| {
            panic!(
                "No downcaster registered for type `{}` in `Views`",
                std::any::type_name::<DbView>(),
            )
        })
    }

    /// Retrieve an downcaster function from `dyn Database` to `dyn DbView`, if it exists.
    #[inline]
    pub fn try_downcaster_for<DbView: ?Sized + Any>(&self) -> Option<DatabaseDownCaster<DbView>> {
        let view_type_id = TypeId::of::<DbView>();
        for (_, view) in self.view_casters.iter() {
            if view.target_type_id == view_type_id {
                // SAFETY: We are unerasing the type erased function pointer having made sure the
                // `TypeId` matches.
                return Some(DatabaseDownCaster(self.source_type_id, unsafe {
                    std::mem::transmute::<ErasedDatabaseDownCasterSig, DatabaseDownCasterSig<DbView>>(
                        view.cast,
                    )
                }));
            }
        }

        None
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
