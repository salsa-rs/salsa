use crate::Database;
use append_only_vec::AppendOnlyVec;
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
    view_casters: Arc<AppendOnlyVec<ViewCaster>>,
}

/// A ViewCaster contains a trait object that can cast from the
/// (ghost) `Db` type of `Views` to some (ghost) `DbView` type.
///
/// You can think of the struct as looking like:
///
/// ```rust,ignore
/// struct ViewCaster<ghost Db, ghost DbView> {
///     target_type_id: TypeId,     // TypeId of DbView
///     type_name: &'static str,    // type name of DbView
///     cast_to: OpaqueBoxDyn,      // a `Box<dyn CastTo<DbView>>` that expects a `Db`
///     free_box: Box<dyn Free>,    // the same box as above, but upcast to `dyn Free`
/// }
/// ```
///
/// As you can see, we have to work very hard to manage things
/// in a way that miri is happy with. What is going on here?
///
/// * The `cast_to` is the cast object, but we can't actually name its type, so
///   we transmute it into some opaque bytes. We can transmute it back once we
///   are in a function monormophized over some function `T` that has the same type-id
///   as `target_type_id`.
/// * The problem is that dropping `cast_to` has no effect and we need
///   to free the box! To do that, we *also* upcast the box to a `Box<dyn Free>`.
///   This trait has no purpose but to carry a destructor.
struct ViewCaster {
    /// The id of the target type `DbView` that we can cast to.
    target_type_id: TypeId,

    /// The name of the target type `DbView` that we can cast to.
    type_name: &'static str,

    /// A "type-obscured" `Box<dyn CastTo<DbView>>`, where `DbView`
    /// is the type whose id is encoded in `target_type_id`.
    cast_to: OpaqueBoxDyn,

    /// An upcasted version of `cast_to`; the only purpose of this field is
    /// to be dropped in the destructor, see `ViewCaster` comment.
    #[allow(dead_code)]
    free_box: Box<dyn Free>,
}

type OpaqueBoxDyn = [u8; std::mem::size_of::<Box<dyn CastTo<Dummy>>>()];

trait CastTo<DbView: ?Sized>: Free {
    /// # Safety requirement
    ///
    /// `db` must have a data pointer whose type is the `Db` type for `Self`
    unsafe fn cast<'db>(&self, db: &'db dyn Database) -> &'db DbView;

    fn into_box_free(self: Box<Self>) -> Box<dyn Free>;
}

trait Free: Send + Sync {}

#[allow(dead_code)]
enum Dummy {}

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        Self {
            source_type_id,
            view_casters: Arc::new(AppendOnlyVec::new()),
        }
    }

    /// Add a new upcast from `Db` to `T`, given the upcasting function `func`.
    pub fn add<Db: Database, DbView: ?Sized + Any>(&self, func: fn(&Db) -> &DbView) {
        assert_eq!(self.source_type_id, TypeId::of::<Db>(), "dyn-upcasts");

        let target_type_id = TypeId::of::<DbView>();

        if self
            .view_casters
            .iter()
            .any(|u| u.target_type_id == target_type_id)
        {
            return;
        }

        let cast_to: Box<dyn CastTo<DbView>> = Box::new(func);
        let cast_to: OpaqueBoxDyn =
            unsafe { std::mem::transmute::<Box<dyn CastTo<DbView>>, OpaqueBoxDyn>(cast_to) };

        // Create a second copy of `cast_to` (which is now `Copy`) and upcast it to a `Box<dyn Any>`.
        // We will drop this box to run the destructor.
        let free_box: Box<dyn Free> = unsafe {
            std::mem::transmute::<OpaqueBoxDyn, Box<dyn CastTo<DbView>>>(cast_to).into_box_free()
        };

        self.view_casters.push(ViewCaster {
            target_type_id,
            type_name: std::any::type_name::<DbView>(),
            cast_to,
            free_box,
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
        for caster in self.view_casters.iter() {
            if caster.target_type_id == view_type_id {
                // SAFETY: We have some function that takes a thin reference to the underlying
                // database type `X` and returns a (potentially wide) reference to `View`.
                //
                // While the compiler doesn't know what `X` is at this point, we know it's the
                // same as the true type of `db_data_ptr`, and the memory representation for `()`
                // and `&X` are the same (since `X` is `Sized`).
                let cast_to: &OpaqueBoxDyn = &caster.cast_to;
                unsafe {
                    let cast_to =
                        std::mem::transmute::<&OpaqueBoxDyn, &Box<dyn CastTo<DbView>>>(cast_to);
                    return Some(cast_to.cast(db));
                };
            }
        }

        None
    }
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynDowncasts")
            .field("vec", &self.view_casters)
            .finish()
    }
}

impl std::fmt::Debug for ViewCaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynDowncast").field(&self.type_name).finish()
    }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `()`).
/// This is safe because `()` gives no access to any data and has no validity requirements in particular.
unsafe fn data_ptr<T: ?Sized, U>(t: &T) -> &U {
    let t: *const T = t;
    let u: *const U = t as *const U;
    unsafe { &*u }
}

impl<Db, DbView> CastTo<DbView> for fn(&Db) -> &DbView
where
    Db: Database,
    DbView: ?Sized + Any,
{
    unsafe fn cast<'db>(&self, db: &'db dyn Database) -> &'db DbView {
        // This tests the safety requirement:
        debug_assert_eq!(db.type_id(), TypeId::of::<Db>());

        // SAFETY:
        //
        // Caller guarantees that the input is of type `Db`
        // (we test it in the debug-assertion above).
        let db = unsafe { data_ptr::<dyn Database, Db>(db) };
        (*self)(db)
    }

    fn into_box_free(self: Box<Self>) -> Box<dyn Free> {
        self
    }
}

impl<T: Send + Sync> Free for T {}
