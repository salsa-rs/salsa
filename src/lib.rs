#![deny(clippy::undocumented_unsafe_blocks)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "accumulator")]
mod accumulator;
mod active_query;
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
mod ingredient_cache;
mod input;
mod interned;
mod key;
mod memo_ingredient_indices;
mod return_mode;
mod revision;
mod runtime;
mod salsa_struct;
mod storage;
mod sync;
mod table;
mod tracing;
mod tracked_struct;
mod update;
mod views;
mod zalsa;
mod zalsa_local;

#[cfg(not(feature = "inventory"))]
mod nonce;

#[cfg(feature = "rayon")]
mod parallel;

#[cfg(feature = "rayon")]
pub use parallel::{join, par_map};
#[cfg(feature = "macros")]
pub use salsa_macros::{accumulator, db, input, interned, tracked, Supertype, Update};

#[cfg(feature = "salsa_unstable")]
pub use self::database::IngredientInfo;

#[cfg(feature = "accumulator")]
pub use self::accumulator::Accumulator;
pub use self::active_query::Backtrace;
pub use self::cancelled::Cancelled;
pub use self::cycle::CycleRecoveryAction;
pub use self::database::Database;
pub use self::database_impl::DatabaseImpl;
pub use self::durability::Durability;
pub use self::event::{Event, EventKind};
pub use self::id::Id;
pub use self::input::setter::Setter;
pub use self::key::DatabaseKeyIndex;
pub use self::return_mode::SalsaAsDeref;
pub use self::return_mode::SalsaAsRef;
pub use self::revision::Revision;
pub use self::runtime::Runtime;
pub use self::storage::{Storage, StorageHandle};
pub use self::update::Update;
pub use self::zalsa::IngredientIndex;
pub use crate::attach::{attach, attach_allow_change, with_attached_database};

pub mod prelude {
    #[cfg(feature = "accumulator")]
    pub use crate::accumulator::Accumulator;
    pub use crate::{Database, Setter};
}

/// Internal names used by salsa macros.
///
/// # WARNING
///
/// The contents of this module are NOT subject to semver.
#[doc(hidden)]
pub mod plumbing {
    pub use std::any::TypeId;
    pub use std::option::Option::{self, None, Some};

    #[cfg(feature = "accumulator")]
    pub use salsa_macro_rules::setup_accumulator_impl;
    pub use salsa_macro_rules::{
        gate_accumulated, macro_if, maybe_backdate, maybe_default, maybe_default_tt,
        return_mode_expression, return_mode_ty, setup_input_struct, setup_interned_struct,
        setup_tracked_assoc_fn_body, setup_tracked_fn, setup_tracked_method_body,
        setup_tracked_struct, unexpected_cycle_initial, unexpected_cycle_recovery,
    };

    #[cfg(feature = "accumulator")]
    pub use crate::accumulator::Accumulator;
    pub use crate::attach::{attach, with_attached_database};
    pub use crate::cycle::{CycleRecoveryAction, CycleRecoveryStrategy};
    pub use crate::database::{current_revision, Database};
    pub use crate::durability::Durability;
    pub use crate::id::{AsId, FromId, FromIdWithDb, Id};
    pub use crate::ingredient::{Ingredient, Jar, Location};
    pub use crate::ingredient_cache::IngredientCache;
    pub use crate::key::DatabaseKeyIndex;
    pub use crate::memo_ingredient_indices::{
        IngredientIndices, MemoIngredientIndices, MemoIngredientMap, MemoIngredientSingletonIndex,
        NewMemoIngredientIndices,
    };
    pub use crate::revision::Revision;
    pub use crate::runtime::{stamp, Runtime, Stamp};
    pub use crate::salsa_struct::SalsaStructInDb;
    pub use crate::storage::{HasStorage, Storage};
    pub use crate::table::memo::MemoTableWithTypes;
    pub use crate::tracked_struct::TrackedStructInDb;
    pub use crate::update::helper::{Dispatch as UpdateDispatch, Fallback as UpdateFallback};
    pub use crate::update::{always_update, Update};
    pub use crate::views::DatabaseDownCaster;
    pub use crate::zalsa::{
        register_jar, transmute_data_ptr, views, ErasedJar, HasJar, IngredientIndex, JarKind,
        Zalsa, ZalsaDatabase,
    };
    pub use crate::zalsa_local::ZalsaLocal;

    #[cfg(feature = "persistence")]
    pub use serde;

    // A stub for `serde` used when persistence is disabled.
    //
    // We provide dummy types to avoid detecting features during macro expansion.
    #[cfg(not(feature = "persistence"))]
    pub mod serde {
        pub trait Serializer {
            type Ok;
            type Error;
        }

        pub trait Deserializer<'de> {
            type Ok;
            type Error;
        }
    }

    #[cfg(feature = "accumulator")]
    pub mod accumulator {
        pub use crate::accumulator::{IngredientImpl, JarImpl};
    }

    pub mod input {
        pub use crate::input::input_field::FieldIngredientImpl;
        pub use crate::input::setter::SetterImpl;
        pub use crate::input::singleton::{NotSingleton, Singleton};
        pub use crate::input::{Configuration, HasBuilder, IngredientImpl, JarImpl, Value};
    }

    pub mod interned {
        pub use crate::interned::{
            Configuration, HashEqLike, IngredientImpl, JarImpl, Lookup, Value,
        };
    }

    pub mod function {
        pub use crate::function::Configuration;
        pub use crate::function::IngredientImpl;
        pub use crate::function::Memo;
        pub use crate::table::memo::MemoEntryType;
    }

    pub mod tracked_struct {
        pub use crate::tracked_struct::tracked_field::FieldIngredientImpl;
        pub use crate::tracked_struct::{Configuration, IngredientImpl, JarImpl, Value};
    }
}
