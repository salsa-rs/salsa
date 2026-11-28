use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::mem;
use std::ptr::NonNull;

use crate::DatabaseKeyIndex;
use crate::function::ErasedMemo;
use crate::sync::atomic::AtomicPtr;
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa::Zalsa;

mod storage;

/// Adds the registered concrete memo type's vtable without dereferencing the pointer.
///
/// Dereferencing the result requires a live, aligned allocation of the same concrete type.
pub(crate) type ToDynMemo = fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>;

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
///
/// The first memo's pointer and query index share one atomic word. If either
/// doesn't fit, or a second query caches a result, the word points to an array
/// with one entry per registered query. The boxed memo stays at its original
/// address so concurrent readers can keep borrowing it.
///
/// # Safety invariant
///
/// The type at each index determines how to drop that memo. The number of
/// types determines the length of the boxed slice to free.
///
/// A page is a block of slots for one ingredient. Each slot has its own
/// `MemoTable`. When Salsa allocates a page, it clones the ingredient's
/// `Arc<MemoTableTypes>`. Insertion, lookup and page destruction use the
/// page's `Arc`. Slot reuse and memory reporting use the ingredient's `Arc`.
/// Both must point to the same immutable list for the lifetime of the page.
#[derive(Default)]
pub struct MemoTable {
    entries: storage::MemoEntries,
}

#[cfg(not(feature = "shuttle"))]
const _: [(); mem::size_of::<MemoTable>()] = [(); mem::size_of::<usize>()];

pub trait Memo: Any + Send + Sync {
    fn has_value(&self) -> bool;

    /// Removes the outputs that were created when this query ran. This includes
    /// tracked structs and specified queries.
    fn remove_outputs(&self, zalsa: &Zalsa, executor: DatabaseKeyIndex);

    /// Returns memory usage information about the memoized value.
    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self) -> crate::database::MemoInfo;
}

/// One pointer in an allocated array. The registered type at this index
/// determines how to restore the concrete memo or its trait object.
#[derive(Default, Debug)]
struct MemoEntry {
    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

/// Type metadata for one memo-table slot.
///
/// Both fields describe the slot's concrete memo type, and the slot stores only that type.
#[derive(Clone, Copy, Debug)]
pub struct MemoEntryType {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A type-coercion function for the erased memo type `M`.
    to_dyn_fn: ToDynMemo,
}

impl MemoEntryType {
    #[inline]
    pub fn of<M: Memo>() -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            to_dyn_fn: Self::to_dyn_fn::<M>(),
        }
    }

    fn to_dummy<M: Memo>(memo: NonNull<M>) -> NonNull<DummyMemo> {
        memo.cast()
    }

    /// Restores a concrete pointer previously erased by [`MemoEntryType::to_dummy`].
    ///
    /// # Safety
    ///
    /// `memo` must have been produced by [`MemoEntryType::to_dummy`] from a live, aligned `M`.
    unsafe fn from_dummy<M: Memo>(memo: NonNull<DummyMemo>) -> NonNull<M> {
        memo.cast()
    }

    pub(crate) const fn to_dyn_fn<M: Memo>() -> ToDynMemo {
        fn to_dyn<M: Memo>(memo: NonNull<DummyMemo>) -> NonNull<dyn Memo> {
            let memo: NonNull<M> = memo.cast();
            memo
        }

        to_dyn::<M>
    }
}

/// Pointee marker for erased memo pointers; never instantiated or dereferenced.
#[derive(Debug)]
pub(crate) struct DummyMemo;

impl Memo for DummyMemo {
    fn has_value(&self) -> bool {
        unreachable!("DummyMemo is never stored in a memo table")
    }

    fn remove_outputs(&self, _zalsa: &Zalsa, _executor: DatabaseKeyIndex) {}

    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self) -> crate::database::MemoInfo {
        crate::database::MemoInfo {
            debug_name: "dummy",
            output: crate::database::SlotInfo {
                debug_name: "dummy",
                size_of_metadata: 0,
                size_of_fields: 0,
                heap_size_of_fields: None,
                memos: Vec::new(),
            },
        }
    }
}

#[derive(Default)]
pub struct MemoTableTypes {
    types: Vec<MemoEntryType>,
}

impl MemoTableTypes {
    pub(crate) fn set(
        &mut self,
        memo_ingredient_index: MemoIngredientIndex,
        memo_type: MemoEntryType,
    ) {
        self.types
            .insert(memo_ingredient_index.as_usize(), memo_type);
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// # Safety
    ///
    /// Every access to `memos` must use the same immutable list of memo types,
    /// as described in [`MemoTable`].
    #[inline]
    pub(crate) unsafe fn attach_memos<'a>(
        &'a self,
        memos: &'a MemoTable,
    ) -> MemoTableWithTypes<'a> {
        MemoTableWithTypes { types: self, memos }
    }

    /// # Safety
    ///
    /// The same requirements as for [`Self::attach_memos`] apply, including
    /// when freeing the entries.
    #[inline]
    pub(crate) unsafe fn attach_memos_mut<'a>(
        &'a self,
        memos: &'a mut MemoTable,
    ) -> MemoTableWithTypesMut<'a> {
        MemoTableWithTypesMut { types: self, memos }
    }
}

/// Borrows one key's memo table and the memo types shared by all keys of its ingredient.
/// The type list determines the concrete memo type at each index and the length of the entry array.
pub struct MemoTableWithTypes<'a> {
    types: &'a MemoTableTypes,
    memos: &'a MemoTable,
}

/// A memo table slot whose observed allocations remain valid for shared access for `'a`.
pub(crate) struct MemoSlot<'a> {
    table: MemoTableWithTypes<'a>,
    memo_ingredient_index: MemoIngredientIndex,
}

impl<'a> MemoSlot<'a> {
    /// Creates a memo slot with an allocation-lifetime guarantee.
    ///
    /// # Safety
    ///
    /// Any allocation observed in this slot must remain at the same address and valid for shared
    /// access for `'a`.
    #[inline]
    pub(crate) unsafe fn new(
        table: MemoTableWithTypes<'a>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Self {
        Self {
            table,
            memo_ingredient_index,
        }
    }

    pub(crate) fn index(&self) -> MemoIngredientIndex {
        self.memo_ingredient_index
    }

    /// Loads the current memo and checks its registered type.
    #[inline]
    pub(crate) fn get<M: Memo>(&self) -> Option<&'a M> {
        let memo = self.table.get(self.memo_ingredient_index)?;
        // SAFETY: The table checks the type and the caller of `new` guarantees
        // the allocation remains valid even when the slot is replaced.
        Some(unsafe { memo.as_ref() })
    }

    /// Loads the erased memo currently stored in this slot.
    #[inline]
    pub(crate) fn get_erased(&self) -> Option<ErasedMemo<'a>> {
        // SAFETY: Guaranteed by the caller of `MemoSlot::new`.
        unsafe { self.table.get_erased(self.memo_ingredient_index) }
    }
}

impl<'a> MemoTableWithTypes<'a> {
    pub(crate) fn insert<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        let index = memo_ingredient_index.as_usize();
        let type_ = self.types.types.get(index)?;

        // Verify that we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        let old_memo = self.store(index, MemoEntryType::to_dummy(memo))?;

        // SAFETY: We asserted that the type is correct above.
        Some(unsafe { MemoEntryType::from_dummy(old_memo) })
    }

    /// Returns a pointer to the memo at the given index, if one has been inserted.
    #[inline]
    pub(crate) fn get<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<M>> {
        let index = memo_ingredient_index.as_usize();
        let memo = self.load(index)?;
        let type_ = &self.types.types[index];

        // Verify that we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        // SAFETY: We asserted that the type is correct above.
        Some(unsafe { MemoEntryType::from_dummy(memo) })
    }

    /// Returns a type-erased view with the slot's registered type metadata.
    ///
    /// # Safety
    ///
    /// Any allocation observed in the table entry must remain at the same address and valid for
    /// shared access for `'a`.
    #[inline]
    unsafe fn get_erased(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ErasedMemo<'a>> {
        let index = memo_ingredient_index.as_usize();
        let memo = self.load(index)?;
        let type_ = &self.types.types[index];

        // SAFETY: `insert` type-checks and release-publishes a pointer to the memo allocation's
        // base address, with spatial provenance covering the allocation, paired with `type_`; the
        // acquire load observes its initialization. The caller guarantees that the allocation
        // remains valid for `'a`.
        Some(unsafe { ErasedMemo::from_raw_parts(memo, type_.to_dyn_fn, type_.type_id) })
    }

    /// Returns the heap size of the entry array in bytes and the memory usage of each memo.
    ///
    /// # Safety
    ///
    /// The caller must ensure exclusive access to the database's storage for the
    /// duration of this call, so memos cannot be replaced or freed during traversal.
    #[cfg(feature = "salsa_unstable")]
    pub(crate) unsafe fn memory_usage(&self) -> (usize, Vec<crate::database::MemoInfo>) {
        self.measure_entries(|index, memo| {
            let type_ = &self.types.types[index];
            // SAFETY: Insertion checked the type. Exclusive database access
            // keeps the memo alive while measuring it.
            unsafe { (type_.to_dyn_fn)(memo).as_ref().memory_usage() }
        })
    }
}

pub(crate) struct MemoTableWithTypesMut<'a> {
    types: &'a MemoTableTypes,
    memos: &'a mut MemoTable,
}

impl MemoTableWithTypesMut<'_> {
    /// Calls `f` on the memo at `memo_ingredient_index`.
    ///
    /// If the memo is not present, `f` is not called.
    pub(crate) fn map_memo<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(&mut M),
    ) {
        let table = MemoTableWithTypes {
            types: self.types,
            memos: self.memos,
        };
        if let Some(mut memo) = table.get::<M>(memo_ingredient_index) {
            // SAFETY: We hold exclusive access to the table and its memos.
            f(unsafe { memo.as_mut() });
        }
    }

    /// To drop an entry, we need its type, so we don't implement `Drop`, and instead have this method.
    ///
    /// Note that calling this multiple times is safe, dropping an uninitialized entry is a no-op.
    ///
    /// # Safety
    ///
    /// The caller needs to make sure to not call this function until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    #[inline]
    pub unsafe fn drop(&mut self) {
        // SAFETY: The caller guarantees that no memo is still borrowed.
        unsafe { self.take_memos(|_, memo| drop(memo)) };
    }

    /// Removes the memos in index order and transfers ownership of each to `f`.
    ///
    /// # Safety
    ///
    /// There must be no outstanding references to the memos.
    pub(crate) unsafe fn take_memos(
        &mut self,
        mut f: impl FnMut(MemoIngredientIndex, Box<dyn Memo>),
    ) {
        let types = self.types;
        self.drain(|index, memo| {
            let type_ = &types.types[index];
            // SAFETY: Drain detached the original boxed memo. Insertion checked
            // its type, and the caller guarantees that no reader still borrows it.
            let memo = unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) };
            f(MemoIngredientIndex::from_usize(index), memo);
        });
    }
}

/// This function is explicitly outlined to avoid debug machinery in the hot-path.
#[cold]
#[inline(never)]
fn type_assert_failed(memo_ingredient_index: MemoIngredientIndex) -> ! {
    panic!("inconsistent type-id for `{memo_ingredient_index:?}`")
}

impl Drop for DummyMemo {
    fn drop(&mut self) {
        unreachable!("should never get here")
    }
}

impl std::fmt::Debug for MemoTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoTable").finish_non_exhaustive()
    }
}
