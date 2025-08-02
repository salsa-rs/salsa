use std::any::{Any, TypeId};
use std::fmt;

use crate::cycle::{
    empty_cycle_heads, CycleHeadKeys, CycleHeads, CycleRecoveryStrategy, IterationCount,
    ProvisionalStatus,
};
use crate::database::RawDatabase;
use crate::function::VerifyResult;
use crate::runtime::Running;
use crate::sync::Arc;
use crate::table::memo::{DeletedEntries, MemoTableTypes};
use crate::table::Table;
use crate::zalsa::{transmute_data_mut_ptr, transmute_data_ptr, IngredientIndex, Zalsa};
use crate::zalsa_local::QueryOriginRef;
use crate::{DatabaseKeyIndex, Id, Revision};

/// A "jar" is a group of ingredients that are added atomically.
///
/// Each type implementing jar can be added to the database at most once.
pub trait Jar: Any {
    /// Create the ingredients given the index of the first one.
    ///
    /// All subsequent ingredients will be assigned contiguous indices.
    fn create_ingredients(
        zalsa: &mut Zalsa,
        first_index: IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>>;

    /// This returns the [`TypeId`] of the ID struct, that is, the struct that wraps `salsa::Id`
    /// and carry the name of the jar.
    fn id_struct_type_id() -> TypeId;
}

pub struct Location {
    pub file: &'static str,
    pub line: u32,
}

pub trait Ingredient: Any + std::fmt::Debug + Send + Sync {
    fn debug_name(&self) -> &'static str;
    fn location(&self) -> &'static Location;

    /// Has the value for `input` in this ingredient changed after `revision`?
    ///
    /// # Safety
    ///
    /// The passed in database needs to be the same one that the ingredient was created with.
    unsafe fn maybe_changed_after(
        &self,
        zalsa: &crate::zalsa::Zalsa,
        db: crate::database::RawDatabase<'_>,
        input: Id,
        revision: Revision,
        cycle_heads: &mut CycleHeadKeys,
    ) -> VerifyResult;

    /// Returns information about the current provisional status of `input`.
    ///
    /// Is it a provisional value or has it been finalized and in which iteration.
    ///
    /// Returns `None` if `input` doesn't exist.
    fn provisional_status(&self, zalsa: &Zalsa, input: Id) -> Option<ProvisionalStatus> {
        _ = (zalsa, input);
        Some(ProvisionalStatus::Final {
            iteration: IterationCount::initial(),
        })
    }

    /// Returns the cycle heads for this ingredient.
    fn cycle_heads<'db>(&self, zalsa: &'db Zalsa, input: Id) -> &'db CycleHeads {
        _ = (zalsa, input);
        empty_cycle_heads()
    }

    /// Invoked when the current thread needs to wait for a result for the given `key_index`.
    /// This call doesn't block the current thread. Instead, it's up to the caller to block
    /// in case `key_index` is [running](`WaitForResult::Running`) on another thread.
    ///
    /// A return value of [`WaitForResult::Available`] indicates that a result is now available.
    /// A return value of [`WaitForResult::Running`] indicates that `key_index` is currently running
    /// on an other thread, it's up to caller to block until the result becomes available if desired.
    /// A return value of [`WaitForResult::Cycle`] means that a cycle was encountered; the waited-on query is either already claimed
    /// by the current thread, or by a thread waiting on the current thread.
    fn wait_for<'me>(&'me self, zalsa: &'me Zalsa, key_index: Id) -> WaitForResult<'me> {
        _ = (zalsa, key_index);
        WaitForResult::Available
    }

    /// Invoked when the value `output_key` should be marked as valid in the current revision.
    /// This occurs because the value for `executor`, which generated it, was marked as valid
    /// in the current revision.
    fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        executor: DatabaseKeyIndex,
        output_key: crate::Id,
    ) {
        let _ = (zalsa, executor, output_key);
        unreachable!("only tracked struct and function ingredients can have validatable outputs")
    }

    /// Invoked when the value `stale_output` was output by `executor` in a previous
    /// revision, but was NOT output in the current revision.
    ///
    /// This hook is used to clear out the stale value so others cannot read it.
    fn remove_stale_output(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex, stale_output_key: Id) {
        let _ = (zalsa, executor, stale_output_key);
        unreachable!("only tracked struct ingredients can have stale outputs")
    }

    /// Returns the [`IngredientIndex`] of this ingredient.
    fn ingredient_index(&self) -> IngredientIndex;

    /// Returns true if `reset_for_new_revision` should be called when new revisions start.
    /// Invoked once when ingredient is added and not after that.
    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    /// Invoked when a new revision is about to start.
    /// This moment is important because it means that we have an `&mut`-reference to the
    /// database, and hence any pre-existing `&`-references must have expired.
    /// Many ingredients, given an `&'db`-reference to the database,
    /// use unsafe code to return `&'db`-references to internal values.
    /// The backing memory for those values can only be freed once an `&mut`-reference to the
    /// database is created.
    ///
    /// **Important:** to actually receive resets, the ingredient must set
    /// [`IngredientRequiresReset::RESET_ON_NEW_REVISION`] to true.
    fn reset_for_new_revision(
        &mut self,
        table: &mut Table,
        new_buffer: DeletedEntries,
    ) -> DeletedEntries {
        _ = (table, new_buffer);
        panic!(
            "Ingredient `{}` set `Ingredient::requires_reset_for_new_revision` to true but does \
            not overwrite `Ingredient::reset_for_new_revision`",
            self.debug_name()
        );
    }

    fn memo_table_types(&self) -> &Arc<MemoTableTypes>;

    fn memo_table_types_mut(&mut self) -> &mut Arc<MemoTableTypes>;

    fn fmt_index(&self, index: crate::Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name(), index, fmt)
    }
    // Function ingredient methods

    /// If this ingredient is a participant in a cycle, what is its cycle recovery strategy?
    /// (Really only relevant to [`crate::function::FunctionIngredient`],
    /// since only function ingredients push themselves onto the active query stack.)
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        unreachable!("only function ingredients can be part of a cycle")
    }

    /// What were the inputs (if any) that were used to create the value at `key_index`.
    fn origin<'db>(&self, zalsa: &'db Zalsa, key_index: Id) -> Option<QueryOriginRef<'db>> {
        let _ = (zalsa, key_index);
        unreachable!("only function ingredients have origins")
    }

    /// What values were accumulated during the creation of the value at `key_index`
    /// (if any).
    ///
    /// # Safety
    ///
    /// The passed in database needs to be the same one that the ingredient was created with.
    #[cfg(feature = "accumulator")]
    unsafe fn accumulated<'db>(
        &'db self,
        db: RawDatabase<'db>,
        key_index: Id,
    ) -> (
        Option<&'db crate::accumulator::accumulated_map::AccumulatedMap>,
        crate::accumulator::accumulated_map::InputAccumulatedValues,
    ) {
        let _ = (db, key_index);
        (
            None,
            crate::accumulator::accumulated_map::InputAccumulatedValues::Empty,
        )
    }

    /// Returns memory usage information about any instances of the ingredient,
    /// if applicable.
    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self, _db: &dyn crate::Database) -> Option<Vec<crate::database::SlotInfo>> {
        None
    }
}

impl dyn Ingredient {
    /// Equivalent to the `downcast` method on `Any`.
    ///
    /// Because we do not have dyn-downcasting support, we need this workaround.
    pub fn assert_type<T: Any>(&self) -> &T {
        assert_eq!(
            self.type_id(),
            TypeId::of::<T>(),
            "ingredient `{self:?}` is not of type `{}`",
            std::any::type_name::<T>()
        );

        // SAFETY: We know that the underlying data pointer
        // refers to a value of type T because of the `TypeId` check above.
        unsafe { transmute_data_ptr(self) }
    }

    /// Equivalent to the `downcast` methods on `Any`.
    ///
    /// Because we do not have dyn-downcasting support, we need this workaround.
    ///
    /// # Safety
    ///
    /// The contained value must be of type `T`.
    pub unsafe fn assert_type_unchecked<T: Any>(&self) -> &T {
        debug_assert_eq!(
            self.type_id(),
            TypeId::of::<T>(),
            "ingredient `{self:?}` is not of type `{}`",
            std::any::type_name::<T>()
        );

        // SAFETY: Guaranteed by caller.
        unsafe { transmute_data_ptr(self) }
    }

    /// Equivalent to the `downcast` method on `Any`.
    ///
    /// Because we do not have dyn-downcasting support, we need this workaround.
    pub fn assert_type_mut<T: Any>(&mut self) -> &mut T {
        assert_eq!(
            Any::type_id(self),
            TypeId::of::<T>(),
            "ingredient `{self:?}` is not of type `{}`",
            std::any::type_name::<T>()
        );

        // SAFETY: We know that the underlying data pointer
        // refers to a value of type T because of the `TypeId` check above.
        unsafe { transmute_data_mut_ptr(self) }
    }
}

/// A helper function to show human readable fmt.
pub(crate) fn fmt_index(debug_name: &str, id: Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(fmt, "{debug_name}({id:?})")
}

pub enum WaitForResult<'me> {
    Running(Running<'me>),
    Available,
    Cycle { same_thread: bool },
}

impl WaitForResult<'_> {
    /// Returns `true` if waiting for this input results in a cycle with another thread.
    pub const fn is_cycle_with_other_thread(&self) -> bool {
        matches!(self, WaitForResult::Cycle { same_thread: false })
    }
}
