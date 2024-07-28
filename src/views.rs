use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use orx_concurrent_vec::ConcurrentVec;

use crate::Database;

#[derive(Clone)]
pub struct Views {
    source_type_id: TypeId,
    view_casters: Arc<ConcurrentVec<ViewCaster>>,
}

struct ViewCaster {
    target_type_id: TypeId,
    type_name: &'static str,
    func: fn(&Dummy) -> &Dummy,
}

#[allow(dead_code)]
enum Dummy {}

impl Views {
    pub(crate) fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        Self {
            source_type_id,
            view_casters: Default::default(),
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

        self.view_casters.push(ViewCaster {
            target_type_id,
            type_name: std::any::type_name::<DbView>(),
            func: unsafe { std::mem::transmute::<fn(&Db) -> &DbView, fn(&Dummy) -> &Dummy>(func) },
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
                let func: fn(&()) -> &DbView = unsafe { std::mem::transmute(caster.func) };
                return Some(func(data_ptr(db)));
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
fn data_ptr<T: ?Sized>(t: &T) -> &() {
    let t: *const T = t;
    let u: *const () = t as *const ();
    unsafe { &*u }
}
