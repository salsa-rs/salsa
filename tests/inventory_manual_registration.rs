#![cfg(feature = "inventory")]

use std::any::TypeId;

use salsa::plumbing::{HasJar, Ingredient, IngredientIndex, Jar, JarKind, Location, Zalsa};

#[salsa::input]
struct RegisteredInput {
    value: u32,
}

struct UnregisteredConfiguration;

impl salsa::plumbing::input::Configuration for UnregisteredConfiguration {
    const DEBUG_NAME: &'static str = "Unregistered";
    const FIELD_DEBUG_NAMES: &'static [&'static str] = &[];
    const LOCATION: Location = Location {
        file: file!(),
        line: line!(),
    };
    const PERSIST: bool = false;

    type Singleton = salsa::plumbing::input::NotSingleton;
    type Struct = salsa::Id;
    type Fields = ();
    type Revisions = [salsa::Revision; 0];
    type Durabilities = [salsa::Durability; 0];

    fn serialize<S: salsa::plumbing::serde::Serializer>(
        _: &Self::Fields,
        _: S,
    ) -> Result<S::Ok, S::Error> {
        unreachable!()
    }

    fn deserialize<'de, D: salsa::plumbing::serde::Deserializer<'de>>(
        _: D,
    ) -> Result<Self::Fields, D::Error> {
        unreachable!()
    }
}

struct UnregisteredJar;

impl Jar for UnregisteredJar {
    fn create_ingredients(_: &mut Zalsa, first: IngredientIndex) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(salsa::plumbing::input::IngredientImpl::<
            UnregisteredConfiguration,
        >::new(first))]
    }

    fn id_struct_type_id() -> TypeId {
        TypeId::of::<salsa::Id>()
    }
}

struct UnregisteredIngredient;

impl HasJar for UnregisteredIngredient {
    type Jar = UnregisteredJar;
    const KIND: JarKind = JarKind::Struct;
}

#[salsa::db]
#[derive(Clone, Default)]
struct DatabaseImpl {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for DatabaseImpl {}

#[test]
#[should_panic(expected = "is not registered with inventory")]
fn rejects_ingredients_not_registered_with_inventory() {
    let _db = DatabaseImpl {
        storage: salsa::Storage::builder()
            .ingredient::<UnregisteredIngredient>()
            .build(),
    };
}

#[test]
fn accepts_redundant_registration_of_inventory_ingredients() {
    let db = DatabaseImpl {
        storage: salsa::Storage::builder()
            .ingredient::<RegisteredInput>()
            .build(),
    };

    let input = RegisteredInput::new(&db, 1);
    assert_eq!(input.value(&db), 1);
}
