//! Basic test of accumulator functionality.

use std::any::{Any, TypeId};
use std::fmt;
use std::marker::PhantomData;
use std::panic::UnwindSafe;
use std::sync::Arc;

use accumulated::{Accumulated, AnyAccumulated};

use crate::function::VerifyResult;
use crate::ingredient::{fmt_index, Ingredient, Jar};
use crate::plumbing::IngredientIndices;
use crate::table::memo::MemoTableTypes;
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Database, Id, Revision};

mod accumulated;
pub(crate) mod accumulated_map;

/// Trait implemented on the struct that user annotated with `#[salsa::accumulator]`.
/// The `Self` type is therefore the types to be accumulated.
pub trait Accumulator: Send + Sync + Any + Sized + UnwindSafe {
    const DEBUG_NAME: &'static str;

    /// Accumulate an instance of this in the database for later retrieval.
    fn accumulate<Db>(self, db: &Db)
    where
        Db: ?Sized + Database;
}

pub struct JarImpl<A: Accumulator> {
    phantom: PhantomData<A>,
}

impl<A: Accumulator> Default for JarImpl<A> {
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}

impl<A: Accumulator> Jar for JarImpl<A> {
    fn create_ingredients(
        _zalsa: &Zalsa,
        first_index: IngredientIndex,
        _dependencies: IngredientIndices,
    ) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(<IngredientImpl<A>>::new(first_index))]
    }

    fn id_struct_type_id() -> TypeId {
        TypeId::of::<A>()
    }
}

pub struct IngredientImpl<A: Accumulator> {
    index: IngredientIndex,
    phantom: PhantomData<Accumulated<A>>,
}

impl<A: Accumulator> IngredientImpl<A> {
    /// Find the accumulator ingredient for `A` in the database, if any.
    pub fn from_db<Db>(db: &Db) -> Option<&Self>
    where
        Db: ?Sized + Database,
    {
        let zalsa = db.zalsa();
        let index = zalsa.add_or_lookup_jar_by_type::<JarImpl<A>>();
        let ingredient = zalsa.lookup_ingredient(index).assert_type::<Self>();
        Some(ingredient)
    }

    pub fn new(index: IngredientIndex) -> Self {
        Self {
            index,
            phantom: PhantomData,
        }
    }

    pub fn push(&self, db: &dyn Database, value: A) {
        let zalsa_local = db.zalsa_local();
        if let Err(()) = zalsa_local.accumulate(self.index, value) {
            panic!("cannot accumulate values outside of an active tracked function");
        }
    }

    pub fn index(&self) -> IngredientIndex {
        self.index
    }
}

impl<A: Accumulator> Ingredient for IngredientImpl<A> {
    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    unsafe fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        _input: Id,
        _revision: Revision,
    ) -> VerifyResult {
        panic!("nothing should ever depend on an accumulator directly")
    }

    fn fmt_index(&self, index: crate::Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(A::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        A::DEBUG_NAME
    }

    fn memo_table_types(&self) -> Arc<MemoTableTypes> {
        unreachable!("accumulator does not allocate pages")
    }
}

impl<A> std::fmt::Debug for IngredientImpl<A>
where
    A: Accumulator,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.index)
            .finish()
    }
}
