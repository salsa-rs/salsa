use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
};

use append_only_vec::AppendOnlyVec;

use crate::Database;

pub(crate) struct DynUpcastsFor<Db: Database> {
    upcasts: DynUpcasts,
    phantom: PhantomData<Db>,
}

#[derive(Clone)]
pub(crate) struct DynUpcasts {
    source_type_id: TypeId,
    vec: Arc<AppendOnlyVec<DynUpcast>>,
}

struct DynUpcast {
    target_type_id: TypeId,
    type_name: &'static str,
    func: fn(&Dummy) -> &Dummy,
    func_mut: fn(&mut Dummy) -> &mut Dummy,
}

#[allow(dead_code)]
enum Dummy {}

impl<Db: Database> Default for DynUpcastsFor<Db> {
    fn default() -> Self {
        Self {
            upcasts: DynUpcasts::new::<Db>(),
            phantom: Default::default(),
        }
    }
}

impl<Db: Database> DynUpcastsFor<Db> {
    /// Add a new upcast from `Db` to `T`, given the upcasting function `func`.
    pub fn add<DbView: ?Sized + Any>(
        &self,
        func: fn(&Db) -> &DbView,
        func_mut: fn(&mut Db) -> &mut DbView,
    ) {
        self.upcasts.add(func, func_mut);
    }
}

impl<Db: Database> Deref for DynUpcastsFor<Db> {
    type Target = DynUpcasts;

    fn deref(&self) -> &Self::Target {
        &self.upcasts
    }
}

impl DynUpcasts {
    fn new<Db: Database>() -> Self {
        let source_type_id = TypeId::of::<Db>();
        Self {
            source_type_id,
            vec: Arc::new(AppendOnlyVec::new()),
        }
    }

    /// Add a new upcast from `Db` to `T`, given the upcasting function `func`.
    fn add<Db: Database, DbView: ?Sized + Any>(
        &self,
        func: fn(&Db) -> &DbView,
        func_mut: fn(&mut Db) -> &mut DbView,
    ) {
        assert_eq!(self.source_type_id, TypeId::of::<Db>(), "dyn-upcasts");

        let target_type_id = TypeId::of::<DbView>();

        if self.vec.iter().any(|u| u.target_type_id == target_type_id) {
            return;
        }

        self.vec.push(DynUpcast {
            target_type_id,
            type_name: std::any::type_name::<DbView>(),
            func: unsafe { std::mem::transmute(func) },
            func_mut: unsafe { std::mem::transmute(func_mut) },
        });
    }

    /// Convert one handle to a salsa database (including a `dyn Database`!) to another.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn try_upcast<'db, DbView: ?Sized + Any>(
        &self,
        db: &'db dyn Database,
    ) -> Option<&'db DbView> {
        let db_type_id = <dyn Database as Any>::type_id(db);
        assert_eq!(self.source_type_id, db_type_id, "database type mismatch");

        let view_type_id = TypeId::of::<DbView>();
        for upcast in self.vec.iter() {
            if upcast.target_type_id == view_type_id {
                // SAFETY: We have some function that takes a thin reference to the underlying
                // database type `X` and returns a (potentially wide) reference to `View`.
                //
                // While the compiler doesn't know what `X` is at this point, we know it's the
                // same as the true type of `db_data_ptr`, and the memory representation for `()`
                // and `&X` are the same (since `X` is `Sized`).
                let func: fn(&()) -> &DbView = unsafe { std::mem::transmute(upcast.func) };
                return Some(func(data_ptr(db)));
            }
        }

        None
    }

    /// Convert one handle to a salsa database (including a `dyn Database`!) to another.
    ///
    /// # Panics
    ///
    /// If the underlying type of `db` is not the same as the database type this upcasts was created for.
    pub fn try_upcast_mut<'db, View: ?Sized + Any>(
        &self,
        db: &'db mut dyn Database,
    ) -> Option<&'db mut View> {
        let db_type_id = <dyn Database as Any>::type_id(db);
        assert_eq!(self.source_type_id, db_type_id, "database type mismatch");

        let view_type_id = TypeId::of::<View>();
        for upcast in self.vec.iter() {
            if upcast.target_type_id == view_type_id {
                // SAFETY: We have some function that takes a thin reference to the underlying
                // database type `X` and returns a (potentially wide) reference to `View`.
                //
                // While the compiler doesn't know what `X` is at this point, we know it's the
                // same as the true type of `db_data_ptr`, and the memory representation for `()`
                // and `&X` are the same (since `X` is `Sized`).
                let func_mut: fn(&mut ()) -> &mut View =
                    unsafe { std::mem::transmute(upcast.func_mut) };
                return Some(func_mut(data_ptr_mut(db)));
            }
        }

        None
    }
}

impl std::fmt::Debug for DynUpcasts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynUpcasts")
            .field("vec", &self.vec)
            .finish()
    }
}

impl std::fmt::Debug for DynUpcast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynUpcast").field(&self.type_name).finish()
    }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `()`).
/// This is safe because `()` gives no access to any data and has no validity requirements in particular.
fn data_ptr<T: ?Sized>(t: &T) -> &() {
    let t: *const T = t;
    let u: *const () = t as *const ();
    unsafe { &*u }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `()`).
/// This is safe because `()` gives no access to any data and has no validity requirements in particular.
fn data_ptr_mut<T: ?Sized>(t: &mut T) -> &mut () {
    let t: *mut T = t;
    let u: *mut () = t as *mut ();
    unsafe { &mut *u }
}

impl<Db: Database> Clone for DynUpcastsFor<Db> {
    fn clone(&self) -> Self {
        Self {
            upcasts: self.upcasts.clone(),
            phantom: self.phantom.clone(),
        }
    }
}
