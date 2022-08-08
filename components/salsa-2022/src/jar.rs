use crate::{
    storage::{HasJar, JarFromJars},
    Database, DbWithJar,
};

use super::routes::Routes;

pub trait Jar<'db>: Sized {
    type DynDb: ?Sized + HasJar<Self> + Database + 'db;

    fn create_jar<DB>(routes: &mut Routes<DB>) -> Self
    where
        DB: JarFromJars<Self> + DbWithJar<Self>;
}
