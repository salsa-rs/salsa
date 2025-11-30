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

use std::borrow::Borrow;
use std::sync::Arc;

use rustc_hash::{FxBuildHasher, FxHashMap};
#[cfg(feature = "macros")]
pub use salsa_macros::{accumulator, db, input, interned, tracked, Supertype, Update};

#[cfg(feature = "salsa_unstable")]
pub use self::database::IngredientInfo;

#[cfg(feature = "accumulator")]
pub use self::accumulator::Accumulator;
pub use self::active_query::Backtrace;
pub use self::cancelled::Cancelled;

pub use self::cycle::Cycle;
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
use crate::plumbing::Ingredient;
use crate::zalsa::{MemoIngredientIndex, Zalsa};

pub mod prelude {
    #[cfg(feature = "accumulator")]
    pub use crate::accumulator::Accumulator;
    pub use crate::{Database, Setter};
}

fn read_u64(bytes: &mut &[u8]) -> u64 {
    let result = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    *bytes = &bytes[8..];
    result
}
fn read_u32(bytes: &mut &[u8]) -> u32 {
    let result = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    *bytes = &bytes[4..];
    result
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct StableIngredientName(Vec<u8>);

impl StableIngredientName {
    fn serialize(&self, output: &mut Vec<u8>) {
        output.extend((self.0.len() as u64).to_le_bytes());
        output.extend_from_slice(&self.0);
    }

    fn deserialize(bytes: &mut &[u8]) -> Self {
        let len = read_u64(bytes) as usize;
        let result = bytes[..len].to_vec();
        *bytes = &bytes[len..];
        Self(result)
    }

    fn from_ingredient(ingredient: &(impl Ingredient + ?Sized)) -> Self {
        Self(ingredient.debug_name().as_bytes().to_vec())
    }
}

impl std::fmt::Debug for StableIngredientName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::str::from_utf8(&self.0).unwrap().fmt(f)
    }
}

impl Borrow<[u8]> for StableIngredientName {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoFrequencyStats {
    best_order: Arc<
        FxHashMap<
            StableIngredientName,
            (
                MemoIngredientIndex,
                FxHashMap<StableIngredientName, MemoIngredientIndex>,
            ),
        >,
    >,
}

impl MemoFrequencyStats {
    pub fn serialize(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend((self.best_order.len() as u64).to_le_bytes());
        for (ingredient, (max_memo_index, memo_indices)) in &*self.best_order {
            ingredient.serialize(&mut output);
            output.extend((max_memo_index.as_usize() as u32).to_le_bytes());
            output.extend((memo_indices.len() as u64).to_le_bytes());
            for (memo_ingredient, &memo_index) in memo_indices {
                memo_ingredient.serialize(&mut output);
                output.extend((memo_index.as_usize() as u32).to_le_bytes());
            }
        }
        output
    }

    pub fn deserialize(mut bytes: &[u8]) -> Self {
        let best_order_len = read_u64(&mut bytes) as usize;
        let mut best_order = FxHashMap::with_capacity_and_hasher(best_order_len, FxBuildHasher);
        for _ in 0..best_order_len {
            let ingredient = StableIngredientName::deserialize(&mut bytes);
            let max_memo_index = MemoIngredientIndex::from_usize(read_u32(&mut bytes) as usize);
            let memo_indices_len = read_u64(&mut bytes) as usize;
            let mut memo_indices =
                FxHashMap::with_capacity_and_hasher(memo_indices_len, FxBuildHasher);
            for _ in 0..memo_indices_len {
                let memo_ingredient = StableIngredientName::deserialize(&mut bytes);
                let memo_index = MemoIngredientIndex::from_usize(read_u32(&mut bytes) as usize);
                memo_indices.insert(memo_ingredient, memo_index);
            }
            best_order.insert(ingredient, (max_memo_index, memo_indices));
        }
        return Self {
            best_order: Arc::new(best_order),
        };
    }

    pub fn determine_from_db(db: &(impl Database + ?Sized)) -> Self {
        Self::determine_from_zalsa(db.zalsa())
    }

    fn determine_from_zalsa(zalsa: &Zalsa) -> Self {
        let mut memo_counts = FxHashMap::default();
        for ingredient in zalsa.ingredients() {
            let counts = ingredient.memo_counts(zalsa);
            if !counts.is_empty() {
                memo_counts.insert(StableIngredientName::from_ingredient(ingredient), counts);
            }
        }
        let best_order = memo_counts
            .into_iter()
            .map(|(ingredient, mut counts)| {
                counts.sort_unstable_by_key(|&(_, count)| std::cmp::Reverse(count));
                let max_memo_index = MemoIngredientIndex::from_usize(counts.len());
                let best_order = counts
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (memo, _))| {
                        (
                            StableIngredientName::from_ingredient(zalsa.lookup_ingredient(memo)),
                            MemoIngredientIndex::from_usize(idx),
                        )
                    })
                    .collect();
                (ingredient, (max_memo_index, best_order))
            })
            .collect();
        Self {
            best_order: Arc::new(best_order),
        }
    }
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
    pub use crate::cycle::CycleRecoveryStrategy;
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
        pub use crate::MemoFrequencyStats;
    }

    pub mod tracked_struct {
        pub use crate::tracked_struct::tracked_field::FieldIngredientImpl;
        pub use crate::tracked_struct::{Configuration, IngredientImpl, JarImpl, Value};
    }
}
