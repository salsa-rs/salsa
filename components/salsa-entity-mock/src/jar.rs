use crate::{
    storage::{HasJar, JarFromJars},
    Database, DbWithJar,
};

use super::routes::Ingredients;

pub trait Jar<'db>: Sized {
    type DynDb: ?Sized + HasJar<Self> + Database + 'db;

    fn create_jar<DB>(ingredients: &mut Ingredients<DB>) -> Self
    where
        DB: JarFromJars<Self> + DbWithJar<Self>;
}
