#![feature(prelude_import)]
#![feature(panic_internals)]
#![feature(fmt_helpers_for_derive)]
#![allow(warnings)]
#![feature(test)]
#![feature(derive_eq)]
#![feature(derive_clone_copy)]
#![feature(core_intrinsics)]
#![feature(structural_match)]
#![feature(coverage_attribute)]
#![feature(rustc_attrs)]
#![feature(raw_ref_op)]

//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.
#[prelude_import]
use std::prelude::rust_2021::*;
#[macro_use]
extern crate std;
use salsa::DebugWithDb;
use test_log::test;
struct Jar(
    <MyInput as salsa::storage::IngredientsFor>::Ingredients,
    <MyTracked<'static> as salsa::storage::IngredientsFor>::Ingredients,
    <create_tracked_list as salsa::storage::IngredientsFor>::Ingredients,
);
impl salsa::storage::HasIngredientsFor<MyInput> for Jar {
    fn ingredient(&self) -> &<MyInput as salsa::storage::IngredientsFor>::Ingredients {
        &self.0
    }
    fn ingredient_mut(&mut self) -> &mut <MyInput as salsa::storage::IngredientsFor>::Ingredients {
        &mut self.0
    }
}
impl salsa::storage::HasIngredientsFor<MyTracked<'_>> for Jar {
    fn ingredient(&self) -> &<MyTracked<'_> as salsa::storage::IngredientsFor>::Ingredients {
        &self.1
    }
    fn ingredient_mut(
        &mut self,
    ) -> &mut <MyTracked<'_> as salsa::storage::IngredientsFor>::Ingredients {
        &mut self.1
    }
}
impl salsa::storage::HasIngredientsFor<create_tracked_list> for Jar {
    fn ingredient(&self) -> &<create_tracked_list as salsa::storage::IngredientsFor>::Ingredients {
        &self.2
    }
    fn ingredient_mut(
        &mut self,
    ) -> &mut <create_tracked_list as salsa::storage::IngredientsFor>::Ingredients {
        &mut self.2
    }
}
unsafe impl<'salsa_db> salsa::jar::Jar<'salsa_db> for Jar {
    type DynDb = dyn Db + 'salsa_db;
    unsafe fn init_jar<DB>(place: *mut Self, routes: &mut salsa::routes::Routes<DB>)
    where
        DB: salsa::storage::JarFromJars<Self> + salsa::storage::DbWithJar<Self>,
    {
        unsafe {
            (&raw mut (*place).0)
                .write(<MyInput as salsa::storage::IngredientsFor>::create_ingredients(routes));
        }
        unsafe {
            (&raw mut (*place).1).write(
                <MyTracked<'_> as salsa::storage::IngredientsFor>::create_ingredients(routes),
            );
        }
        unsafe {
            (&raw mut (*place).2).write(
                <create_tracked_list as salsa::storage::IngredientsFor>::create_ingredients(routes),
            );
        }
    }
}
trait Db: salsa::DbWithJar<Jar> {}
struct Database {
    storage: salsa::Storage<Self>,
}
#[automatically_derived]
impl ::core::default::Default for Database {
    #[inline]
    fn default() -> Database {
        Database {
            storage: ::core::default::Default::default(),
        }
    }
}
impl salsa::database::AsSalsaDatabase for Database {
    fn as_salsa_database(&self) -> &dyn salsa::Database {
        self
    }
}
impl salsa::storage::HasJars for Database {
    type Jars = (Jar,);
    fn jars(&self) -> (&Self::Jars, &salsa::Runtime) {
        self.storage.jars()
    }
    fn jars_mut(&mut self) -> (&mut Self::Jars, &mut salsa::Runtime) {
        self.storage.jars_mut()
    }
    fn create_jars(routes: &mut salsa::routes::Routes<Self>) -> Box<Self::Jars> {
        unsafe {
            salsa::plumbing::create_jars_inplace::<Database>(|jars| unsafe {
                let place = &raw mut (*jars).0;
                <Jar as salsa::jar::Jar>::init_jar(place, routes);
            })
        }
    }
}
impl salsa::storage::HasJarsDyn for Database {
    fn runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
    fn runtime_mut(&mut self) -> &mut salsa::Runtime {
        self.storage.runtime_mut()
    }
    fn maybe_changed_after(
        &self,
        input: salsa::key::DependencyIndex,
        revision: salsa::Revision,
    ) -> bool {
        let ingredient = self.storage.ingredient(input.ingredient_index());
        ingredient.maybe_changed_after(self, input, revision)
    }
    fn cycle_recovery_strategy(
        &self,
        ingredient_index: salsa::IngredientIndex,
    ) -> salsa::cycle::CycleRecoveryStrategy {
        let ingredient = self.storage.ingredient(ingredient_index);
        ingredient.cycle_recovery_strategy()
    }
    fn origin(
        &self,
        index: salsa::DatabaseKeyIndex,
    ) -> Option<salsa::runtime::local_state::QueryOrigin> {
        let ingredient = self.storage.ingredient(index.ingredient_index());
        ingredient.origin(index.key_index())
    }
    fn mark_validated_output(
        &self,
        executor: salsa::DatabaseKeyIndex,
        output: salsa::key::DependencyIndex,
    ) {
        let ingredient = self.storage.ingredient(output.ingredient_index());
        ingredient.mark_validated_output(self, executor, output.key_index());
    }
    fn remove_stale_output(
        &self,
        executor: salsa::DatabaseKeyIndex,
        stale_output: salsa::key::DependencyIndex,
    ) {
        let ingredient = self.storage.ingredient(stale_output.ingredient_index());
        ingredient.remove_stale_output(self, executor, stale_output.key_index());
    }
    fn salsa_struct_deleted(&self, ingredient: salsa::IngredientIndex, id: salsa::Id) {
        let ingredient = self.storage.ingredient(ingredient);
        ingredient.salsa_struct_deleted(self, id);
    }
    fn fmt_index(
        &self,
        index: salsa::key::DependencyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let ingredient = self.storage.ingredient(index.ingredient_index());
        ingredient.fmt_index(index.key_index(), fmt)
    }
}
impl salsa::storage::DbWithJar<Jar> for Database {
    fn as_jar_db<'db>(&'db self) -> &'db <Jar as salsa::jar::Jar<'db>>::DynDb
    where
        'db: 'db,
    {
        self as &'db <Jar as salsa::jar::Jar<'db>>::DynDb
    }
}
impl salsa::storage::HasJar<Jar> for Database {
    fn jar(&self) -> (&Jar, &salsa::Runtime) {
        let (__jars, __runtime) = self.storage.jars();
        (&__jars.0, __runtime)
    }
    fn jar_mut(&mut self) -> (&mut Jar, &mut salsa::Runtime) {
        let (__jars, __runtime) = self.storage.jars_mut();
        (&mut __jars.0, __runtime)
    }
}
impl salsa::storage::JarFromJars<Jar> for Database {
    fn jar_from_jars<'db>(jars: &Self::Jars) -> &Jar {
        &jars.0
    }
    fn jar_from_jars_mut<'db>(jars: &mut Self::Jars) -> &mut Jar {
        &mut jars.0
    }
}
impl salsa::Database for Database {}
impl Db for Database {}
struct MyInput(salsa::Id);
#[automatically_derived]
impl ::core::marker::Copy for MyInput {}
#[automatically_derived]
impl ::core::clone::Clone for MyInput {
    #[inline]
    fn clone(&self) -> MyInput {
        let _: ::core::clone::AssertParamIsClone<salsa::Id>;
        *self
    }
}
#[automatically_derived]
impl ::core::marker::StructuralPartialEq for MyInput {}
#[automatically_derived]
impl ::core::cmp::PartialEq for MyInput {
    #[inline]
    fn eq(&self, other: &MyInput) -> bool {
        self.0 == other.0
    }
}
#[automatically_derived]
impl ::core::cmp::PartialOrd for MyInput {
    #[inline]
    fn partial_cmp(&self, other: &MyInput) -> ::core::option::Option<::core::cmp::Ordering> {
        ::core::cmp::PartialOrd::partial_cmp(&self.0, &other.0)
    }
}
#[automatically_derived]
impl ::core::cmp::Eq for MyInput {
    #[inline]
    #[doc(hidden)]
    #[coverage(off)]
    fn assert_receiver_is_total_eq(&self) -> () {
        let _: ::core::cmp::AssertParamIsEq<salsa::Id>;
    }
}
#[automatically_derived]
impl ::core::cmp::Ord for MyInput {
    #[inline]
    fn cmp(&self, other: &MyInput) -> ::core::cmp::Ordering {
        ::core::cmp::Ord::cmp(&self.0, &other.0)
    }
}
#[automatically_derived]
impl ::core::hash::Hash for MyInput {
    #[inline]
    fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) -> () {
        ::core::hash::Hash::hash(&self.0, state)
    }
}
#[allow(dead_code, clippy::pedantic, clippy::complexity, clippy::style)]
impl MyInput {
    pub fn new(__db: &<crate::Jar as salsa::jar::Jar<'_>>::DynDb, field: String) -> Self {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let __ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyInput>>::ingredient(__jar);
        let __id = __ingredients.1.new_input(__runtime);
        __ingredients
            .0
            .store_new(__runtime, __id, field, salsa::Durability::LOW);
        __id
    }
    fn field<'db>(self, __db: &'db <crate::Jar as salsa::jar::Jar<'_>>::DynDb) -> String {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let __ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyInput>>::ingredient(__jar);
        __ingredients.0.fetch(__runtime, self).clone()
    }
    fn set_field<'db>(
        self,
        __db: &'db mut <crate::Jar as salsa::jar::Jar<'_>>::DynDb,
    ) -> salsa::setter::Setter<'db, MyInput, String> {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar_mut(__db);
        let __ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyInput>>::ingredient_mut(__jar);
        salsa::setter::Setter::new(__runtime, self, &mut __ingredients.0)
    }
    pub fn salsa_id(&self) -> salsa::Id {
        self.0
    }
}
impl salsa::storage::IngredientsFor for MyInput {
    type Jar = crate::Jar;
    type Ingredients = (
        salsa::input_field::InputFieldIngredient<MyInput, String>,
        salsa::input::InputIngredient<MyInput>,
    );
    fn create_ingredients<DB>(routes: &mut salsa::routes::Routes<DB>) -> Self::Ingredients
    where
        DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
    {
        (
            {
                let index = routes.push(
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                        &ingredients.0
                    },
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                        &mut ingredients.0
                    },
                );
                salsa::input_field::InputFieldIngredient::new(index, "field")
            },
            {
                let index = routes.push(
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                        &ingredients.1
                    },
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                        &mut ingredients.1
                    },
                );
                salsa::input::InputIngredient::new(index, "MyInput")
            },
        )
    }
}
impl salsa::id::AsId for MyInput {
    fn as_id(&self) -> salsa::Id {
        self.0
    }
}
impl salsa::id::FromId for MyInput {
    fn from_id(id: salsa::Id) -> Self {
        MyInput(id)
    }
}
impl ::salsa::DebugWithDb<<crate::Jar as salsa::jar::Jar<'_>>::DynDb> for MyInput {
    fn fmt(
        &self,
        f: &mut ::std::fmt::Formatter<'_>,
        _db: &<crate::Jar as salsa::jar::Jar<'_>>::DynDb,
    ) -> ::std::fmt::Result {
        #[allow(unused_imports)]
        use ::salsa::debug::helper::Fallback;
        #[allow(unused_mut)]
        let mut debug_struct = &mut f.debug_struct("MyInput");
        debug_struct = debug_struct.field("[salsa id]", &self.salsa_id().as_u32());
        debug_struct =
            debug_struct.field(
                "field",
                &::salsa::debug::helper::SalsaDebug::<
                    String,
                    <crate::Jar as salsa::jar::Jar<'_>>::DynDb,
                >::salsa_debug(
                    #[allow(clippy::needless_borrow)]
                    &self.field(_db),
                    _db,
                ),
            );
        debug_struct.finish()
    }
}
impl<DB> salsa::salsa_struct::SalsaStructInDb<DB> for MyInput
where
    DB: ?Sized + salsa::DbWithJar<crate::Jar>,
{
    fn register_dependent_fn(_db: &DB, _index: salsa::routes::IngredientIndex) {}
}
impl ::std::fmt::Debug for MyInput {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("MyInput")
            .field("[salsa id]", &self.salsa_id().as_u32())
            .finish()
    }
}
struct __MyTrackedConfig {
    _uninhabited: std::convert::Infallible,
}
impl salsa::tracked_struct::Configuration for __MyTrackedConfig {
    type Fields<'db> = (MyInput, MyList<'db>);
    type Revisions = [salsa::Revision; 2];
    #[allow(clippy::unused_unit)]
    fn id_fields(fields: &Self::Fields<'_>) -> impl std::hash::Hash {
        ()
    }
    fn revision(revisions: &Self::Revisions, field_index: u32) -> salsa::Revision {
        revisions[field_index as usize]
    }
    fn new_revisions(current_revision: salsa::Revision) -> Self::Revisions {
        [current_revision; 2]
    }
    unsafe fn update_fields<'db>(
        current_revision_: salsa::Revision,
        revisions_: &mut Self::Revisions,
        old_fields_: *mut Self::Fields<'db>,
        new_fields_: Self::Fields<'db>,
    ) {
        use salsa::update::helper::Fallback as _;
        if salsa::update::helper::Dispatch::<MyInput>::maybe_update(
            &raw mut (*old_fields_).0,
            new_fields_.0,
        ) {
            revisions_[0] = current_revision_;
        }
        if salsa::update::helper::Dispatch::<MyList<'db>>::maybe_update(
            &raw mut (*old_fields_).1,
            new_fields_.1,
        ) {
            revisions_[1] = current_revision_;
        }
    }
}
struct MyTracked<'db>(
    *const salsa::tracked_struct::ValueStruct<__MyTrackedConfig>,
    std::marker::PhantomData<&'db salsa::tracked_struct::ValueStruct<__MyTrackedConfig>>,
);
#[automatically_derived]
impl<'db> ::core::marker::Copy for MyTracked<'db> {}
#[automatically_derived]
impl<'db> ::core::clone::Clone for MyTracked<'db> {
    #[inline]
    fn clone(&self) -> MyTracked<'db> {
        let _: ::core::clone::AssertParamIsClone<
            *const salsa::tracked_struct::ValueStruct<__MyTrackedConfig>,
        >;
        let _: ::core::clone::AssertParamIsClone<
            std::marker::PhantomData<&'db salsa::tracked_struct::ValueStruct<__MyTrackedConfig>>,
        >;
        *self
    }
}
#[automatically_derived]
impl<'db> ::core::marker::StructuralPartialEq for MyTracked<'db> {}
#[automatically_derived]
impl<'db> ::core::cmp::PartialEq for MyTracked<'db> {
    #[inline]
    fn eq(&self, other: &MyTracked<'db>) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}
#[automatically_derived]
impl<'db> ::core::cmp::PartialOrd for MyTracked<'db> {
    #[inline]
    fn partial_cmp(&self, other: &MyTracked<'db>) -> ::core::option::Option<::core::cmp::Ordering> {
        match ::core::cmp::PartialOrd::partial_cmp(&self.0, &other.0) {
            ::core::option::Option::Some(::core::cmp::Ordering::Equal) => {
                ::core::cmp::PartialOrd::partial_cmp(&self.1, &other.1)
            }
            cmp => cmp,
        }
    }
}
#[automatically_derived]
impl<'db> ::core::cmp::Eq for MyTracked<'db> {
    #[inline]
    #[doc(hidden)]
    #[coverage(off)]
    fn assert_receiver_is_total_eq(&self) -> () {
        let _: ::core::cmp::AssertParamIsEq<
            *const salsa::tracked_struct::ValueStruct<__MyTrackedConfig>,
        >;
        let _: ::core::cmp::AssertParamIsEq<
            std::marker::PhantomData<&'db salsa::tracked_struct::ValueStruct<__MyTrackedConfig>>,
        >;
    }
}
#[automatically_derived]
impl<'db> ::core::cmp::Ord for MyTracked<'db> {
    #[inline]
    fn cmp(&self, other: &MyTracked<'db>) -> ::core::cmp::Ordering {
        match ::core::cmp::Ord::cmp(&self.0, &other.0) {
            ::core::cmp::Ordering::Equal => ::core::cmp::Ord::cmp(&self.1, &other.1),
            cmp => cmp,
        }
    }
}
#[automatically_derived]
impl<'db> ::core::hash::Hash for MyTracked<'db> {
    #[inline]
    fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) -> () {
        ::core::hash::Hash::hash(&self.0, state);
        ::core::hash::Hash::hash(&self.1, state)
    }
}
#[allow(dead_code, clippy::pedantic, clippy::complexity, clippy::style)]
impl<'db> MyTracked<'db> {
    pub fn new(
        __db: &'db <crate::Jar as salsa::jar::Jar<'db>>::DynDb,
        data: MyInput,
        next: MyList<'db>,
    ) -> Self {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let __ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<Self>>::ingredient(__jar);
        let __data = __ingredients.0.new_struct(__runtime, (data, next));
        Self(__data, std::marker::PhantomData)
    }
    pub fn salsa_id(&self) -> salsa::Id {
        salsa::id::AsId::as_id(unsafe { &*self.0 })
    }
    fn data(self, __db: &'db <crate::Jar as salsa::jar::Jar<'db>>::DynDb) -> MyInput {
        let (_, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let fields = unsafe { &*self.0 }.field(__runtime, 0);
        fields.0.clone()
    }
    fn next(self, __db: &'db <crate::Jar as salsa::jar::Jar<'db>>::DynDb) -> MyList<'db> {
        let (_, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let fields = unsafe { &*self.0 }.field(__runtime, 1);
        fields.1.clone()
    }
}
impl<'db> salsa::storage::IngredientsFor for MyTracked<'db> {
    type Jar = crate::Jar;
    type Ingredients = (
        salsa::tracked_struct::TrackedStructIngredient<__MyTrackedConfig>,
        [salsa::tracked_struct::TrackedFieldIngredient<__MyTrackedConfig>; 2],
    );
    fn create_ingredients<DB>(routes: &mut salsa::routes::Routes<DB>) -> Self::Ingredients
    where
        DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
    {
        let struct_ingredient = {
            let index = routes.push(
                |jars| {
                    let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                    let ingredients =
                        <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                    &ingredients.0
                },
                |jars| {
                    let jar =
                        <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                    let ingredients =
                        <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                    &mut ingredients.0
                },
            );
            salsa::tracked_struct::TrackedStructIngredient::new(index, "MyTracked")
        };
        let field_ingredients = [
            {
                let index = routes.push(
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                        &ingredients.1[0]
                    },
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                        &mut ingredients.1[0]
                    },
                );
                struct_ingredient.new_field_ingredient(index, 0, "data")
            },
            {
                let index = routes.push(
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient(jar);
                        &ingredients.1[1]
                    },
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                        let ingredients =
                            <_ as salsa::storage::HasIngredientsFor<Self>>::ingredient_mut(jar);
                        &mut ingredients.1[1]
                    },
                );
                struct_ingredient.new_field_ingredient(index, 1, "next")
            },
        ];
        (struct_ingredient, field_ingredients)
    }
}
impl<'db, DB> salsa::salsa_struct::SalsaStructInDb<DB> for MyTracked<'db>
where
    DB: ?Sized + salsa::DbWithJar<crate::Jar>,
{
    fn register_dependent_fn(db: &DB, index: salsa::routes::IngredientIndex) {
        let (jar, _) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(db);
        let ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyTracked<'db>>>::ingredient(jar);
        ingredients.0.register_dependent_fn(index)
    }
}
impl<'db, DB> salsa::tracked_struct::TrackedStructInDb<DB> for MyTracked<'db>
where
    DB: ?Sized + salsa::DbWithJar<crate::Jar>,
{
    fn database_key_index(db: &DB, id: salsa::Id) -> salsa::DatabaseKeyIndex {
        let (jar, _) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(db);
        let ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyTracked<'db>>>::ingredient(jar);
        ingredients.0.database_key_index(id)
    }
}
unsafe impl<'db> salsa::update::Update for MyTracked<'db> {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        if unsafe { *old_pointer } != new_value {
            unsafe { *old_pointer = new_value };
            true
        } else {
            false
        }
    }
}
impl<'db> salsa::id::AsId for MyTracked<'db> {
    fn as_id(&self) -> salsa::Id {
        salsa::id::AsId::as_id(unsafe { &*self.0 })
    }
}
unsafe impl<'db> std::marker::Send for MyTracked<'db> {}
unsafe impl<'db> std::marker::Sync for MyTracked<'db> {}
impl<'db, DB> salsa::id::LookupId<&'db DB> for MyTracked<'db>
where
    DB: ?Sized + salsa::DbWithJar<crate::Jar>,
{
    fn lookup_id(id: salsa::Id, db: &'db DB) -> Self {
        let (jar, runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(db);
        let ingredients =
            <crate::Jar as salsa::storage::HasIngredientsFor<MyTracked<'db>>>::ingredient(jar);
        Self(
            ingredients.0.lookup_struct(runtime, id),
            std::marker::PhantomData,
        )
    }
}
impl<'db> ::salsa::DebugWithDb<<crate::Jar as salsa::jar::Jar<'db>>::DynDb> for MyTracked<'db> {
    fn fmt(
        &self,
        f: &mut ::std::fmt::Formatter<'_>,
        _db: &<crate::Jar as salsa::jar::Jar<'db>>::DynDb,
    ) -> ::std::fmt::Result {
        #[allow(unused_imports)]
        use ::salsa::debug::helper::Fallback;
        #[allow(unused_mut)]
        let mut debug_struct = &mut f.debug_struct("MyTracked");
        debug_struct = debug_struct.field("[salsa id]", &self.salsa_id().as_u32());
        debug_struct = debug_struct.field(
            "data",
            &::salsa::debug::helper::SalsaDebug::<
                MyInput,
                <crate::Jar as salsa::jar::Jar<'_>>::DynDb,
            >::salsa_debug(
                #[allow(clippy::needless_borrow)]
                &self.data(_db),
                _db,
            ),
        );
        debug_struct = debug_struct.field(
            "next",
            &::salsa::debug::helper::SalsaDebug::<
                MyList<'_>,
                <crate::Jar as salsa::jar::Jar<'_>>::DynDb,
            >::salsa_debug(
                #[allow(clippy::needless_borrow)]
                &self.next(_db),
                _db,
            ),
        );
        debug_struct.finish()
    }
}
impl<'db> ::std::fmt::Debug for MyTracked<'db> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("MyTracked")
            .field("[salsa id]", &self.salsa_id().as_u32())
            .finish()
    }
}
enum MyList<'db> {
    None,
    Next(MyTracked<'db>),
}
#[automatically_derived]
impl<'db> ::core::marker::StructuralPartialEq for MyList<'db> {}
#[automatically_derived]
impl<'db> ::core::cmp::PartialEq for MyList<'db> {
    #[inline]
    fn eq(&self, other: &MyList<'db>) -> bool {
        let __self_discr = ::core::intrinsics::discriminant_value(self);
        let __arg1_discr = ::core::intrinsics::discriminant_value(other);
        __self_discr == __arg1_discr
            && match (self, other) {
                (MyList::Next(__self_0), MyList::Next(__arg1_0)) => __self_0 == __arg1_0,
                _ => true,
            }
    }
}
#[automatically_derived]
impl<'db> ::core::cmp::Eq for MyList<'db> {
    #[inline]
    #[doc(hidden)]
    #[coverage(off)]
    fn assert_receiver_is_total_eq(&self) -> () {
        let _: ::core::cmp::AssertParamIsEq<MyTracked<'db>>;
    }
}
#[automatically_derived]
impl<'db> ::core::clone::Clone for MyList<'db> {
    #[inline]
    fn clone(&self) -> MyList<'db> {
        match self {
            MyList::None => MyList::None,
            MyList::Next(__self_0) => MyList::Next(::core::clone::Clone::clone(__self_0)),
        }
    }
}
#[automatically_derived]
impl<'db> ::core::fmt::Debug for MyList<'db> {
    #[inline]
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        match self {
            MyList::None => ::core::fmt::Formatter::write_str(f, "None"),
            MyList::Next(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(f, "Next", &__self_0)
            }
        }
    }
}
unsafe impl<'db> salsa::update::Update for MyList<'db> {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        use ::salsa::update::helper::Fallback as _;
        let old_pointer = unsafe { &mut *old_pointer };
        match old_pointer {
            MyList::None => {
                let new_value = if let MyList::None = new_value {
                    ()
                } else {
                    *old_pointer = new_value;
                    return true;
                };
                false
            }
            MyList::Next(__binding_0) => {
                let new_value = if let MyList::Next(__binding_0) = new_value {
                    (__binding_0,)
                } else {
                    *old_pointer = new_value;
                    return true;
                };
                false
                    | unsafe {
                        salsa::update::helper::Dispatch::<MyTracked<'db>>::maybe_update(
                            __binding_0,
                            new_value.0,
                        )
                    }
            }
        }
    }
}
const _: () = {
    impl<'db> ::salsa::debug::DebugWithDb<<crate::Jar as salsa::jar::Jar<'db>>::DynDb> for MyList<'db> {
        fn fmt(
            &self,
            fmt: &mut std::fmt::Formatter<'_>,
            db: &<crate::Jar as salsa::jar::Jar<'db>>::DynDb,
        ) -> std::fmt::Result {
            use ::salsa::debug::helper::Fallback as _;
            match self {
                MyList::None => { fmt.debug_tuple("None") }.finish(),
                MyList::Next(ref __binding_0) => {
                    fmt.debug_tuple("Next")
                        .field(&::salsa::debug::helper::SalsaDebug::<
                            MyTracked<'db>,
                            <crate::Jar as salsa::jar::Jar<'db>>::DynDb,
                        >::salsa_debug(__binding_0, db))
                }
                .finish(),
            }
        }
    }
};
#[allow(non_camel_case_types)]
struct create_tracked_list {
    intern_map: salsa::interned::IdentityInterner<Self>,
    function: salsa::function::FunctionIngredient<Self>,
}
impl salsa::function::Configuration for create_tracked_list {
    type Jar = crate::Jar;
    type SalsaStruct<'db> = MyInput;
    type Input<'db> = MyInput;
    type Value<'db> = MyTracked<'db>;
    const CYCLE_STRATEGY: salsa::cycle::CycleRecoveryStrategy =
        salsa::cycle::CycleRecoveryStrategy::Panic;
    fn should_backdate_value(v1: &Self::Value<'_>, v2: &Self::Value<'_>) -> bool {
        salsa::function::should_backdate_value(v1, v2)
    }
    fn execute<'db>(
        __db: &'db salsa::function::DynDb<'db, Self>,
        __id: salsa::Id,
    ) -> Self::Value<'db> {
        fn __fn<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
            let t0 = MyTracked::new(db, input, MyList::None);
            let t1 = MyTracked::new(db, input, MyList::Next(t0));
            t1
        }
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(__db);
        let __ingredients =
            <_ as salsa::storage::HasIngredientsFor<create_tracked_list>>::ingredient(__jar);
        let __key = __ingredients.intern_map.data_with_db(__id, __db).clone();
        __fn(__db, __key)
    }
    fn recover_from_cycle<'db>(
        _db: &'db salsa::function::DynDb<'db, Self>,
        _cycle: &salsa::Cycle,
        _key: salsa::Id,
    ) -> Self::Value<'db> {
        {
            #[cold]
            #[track_caller]
            #[inline(never)]
            const fn panic_cold_explicit() -> ! {
                ::core::panicking::panic_explicit()
            }
            panic_cold_explicit();
        }
    }
}
impl salsa::interned::Configuration for create_tracked_list {
    type Data<'db> = (MyInput);
}
impl salsa::storage::IngredientsFor for create_tracked_list {
    type Ingredients = Self;
    type Jar = crate::Jar;
    fn create_ingredients<DB>(routes: &mut salsa::routes::Routes<DB>) -> Self::Ingredients
    where
        DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
    {
        Self {
            intern_map: salsa::interned::IdentityInterner::new(),
            function: {
                let index = routes.push(
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                        let ingredients = <_ as salsa::storage::HasIngredientsFor<
                            Self::Ingredients,
                        >>::ingredient(jar);
                        &ingredients.function
                    },
                    |jars| {
                        let jar =
                            <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                        let ingredients = <_ as salsa::storage::HasIngredientsFor<
                            Self::Ingredients,
                        >>::ingredient_mut(jar);
                        &mut ingredients.function
                    },
                );
                let ingredient =
                    salsa::function::FunctionIngredient::new(index, "create_tracked_list");
                ingredient.set_capacity(0usize);
                ingredient
            },
        }
    }
}
impl create_tracked_list {
    #[allow(dead_code, clippy::needless_lifetimes)]
    fn get<'db>(db: &'db dyn Db, input: MyInput) -> &'db MyTracked<'db> {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(db);
        let __ingredients =
            <_ as salsa::storage::HasIngredientsFor<create_tracked_list>>::ingredient(__jar);
        let __key = __ingredients.intern_map.intern_id(__runtime, (input));
        __ingredients.function.fetch(db, __key)
    }
    #[allow(dead_code, clippy::needless_lifetimes)]
    fn set<'db>(db: &'db mut dyn Db, input: MyInput, __value: MyTracked<'db>) {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar_mut(db);
        let __ingredients =
            <_ as salsa::storage::HasIngredientsFor<create_tracked_list>>::ingredient_mut(__jar);
        let __key = __ingredients.intern_map.intern_id(__runtime, (input));
        __ingredients
            .function
            .store(__runtime, __key, __value, salsa::Durability::LOW)
    }
    #[allow(dead_code, clippy::needless_lifetimes)]
    fn accumulated<'db, __A: salsa::accumulator::Accumulator>(
        db: &'db dyn Db,
        input: MyInput,
    ) -> Vec<<__A as salsa::accumulator::Accumulator>::Data>
    where
        <crate::Jar as salsa::jar::Jar<'db>>::DynDb:
            salsa::storage::HasJar<<__A as salsa::accumulator::Accumulator>::Jar>,
    {
        let (__jar, __runtime) = <_ as salsa::storage::HasJar<crate::Jar>>::jar(db);
        let __ingredients =
            <_ as salsa::storage::HasIngredientsFor<create_tracked_list>>::ingredient(__jar);
        let __key = __ingredients.intern_map.intern_id(__runtime, (input));
        __ingredients.function.accumulated::<__A>(db, __key)
    }
}
#[allow(clippy::needless_lifetimes)]
fn create_tracked_list<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
    Clone::clone(create_tracked_list::get(db, input))
}
extern crate test;
#[cfg(test)]
#[rustc_test_marker = "execute"]
pub const execute: test::TestDescAndFn = test::TestDescAndFn {
    desc: test::TestDesc {
        name: test::StaticTestName("execute"),
        ignore: false,
        ignore_message: ::core::option::Option::None,
        source_file: "salsa-2022-tests/tests/tracked_with_struct_db.rs",
        start_line: 47usize,
        start_col: 4usize,
        end_line: 47usize,
        end_col: 11usize,
        compile_fail: false,
        no_run: false,
        should_panic: test::ShouldPanic::No,
        test_type: test::TestType::IntegrationTest,
    },
    testfn: test::StaticTestFn(
        #[coverage(off)]
        || test::assert_test_result(execute1()),
    ),
};
fn execute1() {
    mod init {
        pub fn init() {
            {
                let mut env_logger_builder = ::test_log::env_logger::builder();
                let _ = env_logger_builder.is_test(true).try_init();
            }
        }
    }
    init::init();
    {
        let mut db = Database::default();
        let input = MyInput::new(&mut db, "foo".to_string());
        let t0: MyTracked = create_tracked_list(&db, input);
        let t1 = create_tracked_list(&db, input);
        ::expect_test::Expect {
            position: ::expect_test::Position {
                file: "salsa-2022-tests/tests/tracked_with_struct_db.rs",
                line: 52u32,
                column: 5u32,
            },
            data: r#"
        MyTracked {
            [salsa id]: 1,
            data: MyInput {
                [salsa id]: 0,
                field: "foo",
            },
            next: Next(
                MyTracked {
                    [salsa id]: 0,
                    data: MyInput {
                        [salsa id]: 0,
                        field: "foo",
                    },
                    next: None,
                },
            ),
        }
    "#,
            indent: true,
        }
        .assert_debug_eq(&t0.debug(&db));
        match (&t0, &t1) {
            (left_val, right_val) => {
                if !(*left_val == *right_val) {
                    let kind = ::core::panicking::AssertKind::Eq;
                    ::core::panicking::assert_failed(
                        kind,
                        &*left_val,
                        &*right_val,
                        ::core::option::Option::None,
                    );
                }
            }
        };
    }
}
#[rustc_main]
#[coverage(off)]
pub fn main() -> () {
    extern crate test;
    test::test_main_static(&[&execute])
}
