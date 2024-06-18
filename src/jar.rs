use crate::{
    storage::{HasJar, JarFromJars},
    Database, DbWithJar,
};

use super::routes::Routes;

/// Representative trait of a salsa jar
///
/// # Safety
///
/// `init_jar` must fully initialize the jar
pub unsafe trait Jar<'db>: Sized {
    type DynDb: ?Sized + HasJar<Self> + Database + 'db;

    /// Initializes the jar at `place`
    ///
    /// # Safety
    ///
    /// `place` must be a valid pointer to this jar
    unsafe fn init_jar<DB>(place: *mut Self, routes: &mut Routes<DB>)
    where
        DB: JarFromJars<Self> + DbWithJar<Self>;
}
