mod accumulator;
mod active_query;
mod array;
mod attach;
mod cancelled;
mod cycle;
mod database;
mod database_impl;
mod durability;
mod event;
mod function;
mod hash;
mod id;
mod ingredient;
mod input;
mod interned;
mod key;
mod nonce;
mod revision;
mod runtime;
mod salsa_struct;
mod storage;
mod table;
mod tracked_struct;
mod update;
mod views;
mod zalsa;
mod zalsa_local;

pub use self::accumulator::Accumulator;
pub use self::cancelled::Cancelled;
pub use self::cycle::CycleRecoveryAction;
pub use self::database::AsDynDatabase;
pub use self::database::Database;
pub use self::database_impl::DatabaseImpl;
pub use self::durability::Durability;
pub use self::event::Event;
pub use self::event::EventKind;
pub use self::id::Id;
pub use self::input::setter::Setter;
pub use self::key::DatabaseKeyIndex;
pub use self::revision::Revision;
pub use self::runtime::Runtime;
pub use self::storage::Storage;
pub use self::update::Update;
pub use self::zalsa::IngredientIndex;
pub use crate::attach::with_attached_database;
pub use salsa_macros::accumulator;
pub use salsa_macros::db;
pub use salsa_macros::input;
pub use salsa_macros::interned;
pub use salsa_macros::tracked;
pub use salsa_macros::Update;

pub mod prelude {
    pub use crate::Accumulator;
    pub use crate::Database;
    pub use crate::Setter;
}

/// Internal names used by salsa macros.
///
/// # WARNING
///
/// The contents of this module are NOT subject to semver.
pub mod plumbing {
    pub use crate::accumulator::Accumulator;
    pub use crate::array::Array;
    pub use crate::attach::attach;
    pub use crate::attach::with_attached_database;
    pub use crate::cycle::CycleRecoveryAction;
    pub use crate::cycle::CycleRecoveryStrategy;
    pub use crate::database::current_revision;
    pub use crate::database::Database;
    pub use crate::function::should_backdate_value;
    pub use crate::id::AsId;
    pub use crate::id::FromId;
    pub use crate::id::Id;
    pub use crate::ingredient::Ingredient;
    pub use crate::ingredient::Jar;
    pub use crate::ingredient::JarAux;
    pub use crate::key::DatabaseKeyIndex;
    pub use crate::revision::Revision;
    pub use crate::runtime::stamp;
    pub use crate::runtime::Runtime;
    pub use crate::runtime::Stamp;
    pub use crate::runtime::StampedValue;
    pub use crate::salsa_struct::SalsaStructInDb;
    pub use crate::storage::HasStorage;
    pub use crate::storage::Storage;
    pub use crate::tracked_struct::TrackedStructInDb;
    pub use crate::update::always_update;
    pub use crate::update::helper::Dispatch as UpdateDispatch;
    pub use crate::update::helper::Fallback as UpdateFallback;
    pub use crate::update::Update;
    pub use crate::zalsa::views;
    pub use crate::zalsa::IngredientCache;
    pub use crate::zalsa::IngredientIndex;
    pub use crate::zalsa::Zalsa;
    pub use crate::zalsa::ZalsaDatabase;
    pub use crate::zalsa_local::ZalsaLocal;

    pub use salsa_macro_rules::macro_if;
    pub use salsa_macro_rules::maybe_backdate;
    pub use salsa_macro_rules::maybe_clone;
    pub use salsa_macro_rules::maybe_cloned_ty;
    pub use salsa_macro_rules::maybe_default;
    pub use salsa_macro_rules::maybe_default_tt;
    pub use salsa_macro_rules::setup_accumulator_impl;
    pub use salsa_macro_rules::setup_input_struct;
    pub use salsa_macro_rules::setup_interned_struct;
    pub use salsa_macro_rules::setup_method_body;
    pub use salsa_macro_rules::setup_tracked_fn;
    pub use salsa_macro_rules::setup_tracked_struct;
    pub use salsa_macro_rules::unexpected_cycle_initial;
    pub use salsa_macro_rules::unexpected_cycle_recovery;

    pub mod accumulator {
        pub use crate::accumulator::IngredientImpl;
        pub use crate::accumulator::JarImpl;
    }

    pub mod input {
        pub use crate::input::input_field::FieldIngredientImpl;
        pub use crate::input::setter::SetterImpl;
        pub use crate::input::Configuration;
        pub use crate::input::HasBuilder;
        pub use crate::input::IngredientImpl;
        pub use crate::input::JarImpl;
    }

    pub mod interned {
        pub use crate::interned::Configuration;
        pub use crate::interned::IngredientImpl;
        pub use crate::interned::JarImpl;
        pub use crate::interned::Lookup;
        pub use crate::interned::Value;
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
        pub use crate::tracked_struct::Value;
    }
}
