mod accumulator;
mod alloc;
mod cancelled;
mod cycle;
mod database;
mod durability;
mod event;
mod function;
mod hash;
mod id;
mod ingredient;
mod ingredient_list;
mod input;
mod input_field;
mod interned;
mod key;
mod nonce;
mod revision;
mod runtime;
mod salsa_struct;
mod setter;
mod storage;
mod tracked_struct;
mod update;
mod views;

pub use self::cancelled::Cancelled;
pub use self::cycle::Cycle;
pub use self::database::Database;
pub use self::database::ParallelDatabase;
pub use self::database::Snapshot;
pub use self::durability::Durability;
pub use self::event::Event;
pub use self::event::EventKind;
pub use self::id::Id;
pub use self::key::DatabaseKeyIndex;
pub use self::revision::Revision;
pub use self::runtime::Runtime;
pub use self::storage::Storage;
pub use salsa_macros::accumulator;
pub use salsa_macros::db;
pub use salsa_macros::input;
pub use salsa_macros::interned;
pub use salsa_macros::tracked;
pub use salsa_macros::DebugWithDb;
pub use salsa_macros::Update;

/// Internal names used by salsa macros.
///
/// # WARNING
///
/// The contents of this module are NOT subject to semver.
pub mod plumbing {
    pub use crate::cycle::Cycle;
    pub use crate::cycle::CycleRecoveryStrategy;
    pub use crate::database::Database;
    pub use crate::id::AsId;
    pub use crate::id::FromId;
    pub use crate::id::Id;
    pub use crate::ingredient::Ingredient;
    pub use crate::ingredient::Jar;
    pub use crate::salsa_struct::SalsaStructInDb;
    pub use crate::storage::views;
    pub use crate::storage::HasStorage;
    pub use crate::storage::IngredientCache;
    pub use crate::storage::IngredientIndex;
    pub use crate::storage::Storage;

    pub mod input {
        pub use crate::input::Configuration;
        pub use crate::input::IngredientImpl;
        pub use crate::input_field::FieldIngredientImpl;
    }

    pub mod interned {
        pub use crate::interned::Configuration;
        pub use crate::interned::IngredientImpl;
        pub use crate::interned::ValueStruct;
    }

    pub mod function {
        pub use crate::function::Configuration;
        pub use crate::function::IngredientImpl;
    }

    pub mod tracked_struct {
        pub use crate::tracked_struct::tracked_field::FieldIngredientImpl;
        pub use crate::tracked_struct::Configuration;
        pub use crate::tracked_struct::IngredientImpl;
        pub use crate::tracked_struct::JarImpl;
    }
}
