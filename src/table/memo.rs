use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::mem;
use std::ptr::{self, NonNull};

use crate::DatabaseKeyIndex;
use crate::function::ErasedMemo;
use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa::Zalsa;

/// Adds the registered concrete memo type's vtable without dereferencing the pointer.
///
/// Dereferencing the result requires a live, aligned allocation of the same concrete type.
pub(crate) type ToDynMemo = fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>;

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
///
/// # Safety invariant
///
/// The first insertion allocates one entry per registered memo type. The type
/// at each index determines how to drop that memo, and the number of types
/// determines the length of the boxed slice to free.
///
/// Lookup and page destruction use the page's `MemoTableTypes`. Slot reuse
/// and memory reporting use the ingredient's. Both must refer to the same
/// immutable type table for the lifetime of the page.
#[derive(Default)]
pub struct MemoTable {
    entries: AtomicPtr<MemoEntry>,
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

/// Data for a memoized entry.
/// This is a type-erased `Box<M>`, where `M` is the type of memo associated
/// with that particular ingredient index.
///
/// # Implementation note
///
/// Every entry is associated with some ingredient that has been added to the database.
/// That ingredient has a fixed type of values that it produces etc.
/// Therefore, once a given entry goes from `Empty` to `Full`,
/// the type-id associated with that entry should never change.
///
/// We take advantage of this and use an `AtomicPtr` to store the actual memo.
/// This allows us to store into the memo-entry without acquiring a write-lock.
/// However, using `AtomicPtr` means we cannot use a `Box<dyn Any>` or any other wide pointer.
/// Therefore, we hide the type by transmuting to `DummyMemo`; but we must then be very careful
/// when freeing `MemoEntryData` values to transmute things back. See [`MemoEntry::take`].
#[derive(Default, Debug)]
struct MemoEntry {
    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

impl MemoTableWithTypes<'_> {
    #[inline]
    fn entries(&self) -> Option<&[MemoEntry]> {
        let ptr = NonNull::new(self.memos.entries.load(Ordering::Acquire))?;

        // The acquire load synchronizes with the release operation that published the pointer,
        // ensuring that the memo entries are initialized before we create references to them.
        //
        // SAFETY: A non-null pointer comes from a boxed slice of length `self.types.len()` and
        // cannot be freed while `self` is shared.
        Some(unsafe { std::slice::from_raw_parts(ptr.as_ptr(), self.types.len()) })
    }

    #[cold]
    fn initialize(&self) -> &[MemoEntry] {
        let len = self.types.len();
        let new_memos: Box<[MemoEntry]> = (0..len).map(|_| MemoEntry::default()).collect();
        let new_memos = Box::into_raw(new_memos);
        let new_memos_ptr = new_memos.cast::<MemoEntry>();

        // Release publishes the initialized memo entries. If another thread won the race, acquire
        // synchronizes with its release operation before we create references to its allocation.
        let ptr = match self.memos.entries.compare_exchange(
            ptr::null_mut(),
            new_memos_ptr,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => new_memos_ptr,
            Err(ptr) => {
                // SAFETY: The compare-exchange failed, so `new_memos` was not published and this
                // thread retains ownership of the allocation.
                unsafe { drop(Box::from_raw(new_memos)) };
                ptr
            }
        };

        // SAFETY: `ptr` is either the boxed slice allocated above or the boxed slice published by
        // another thread. Both allocations have length `len` and cannot be freed while `self`
        // is shared.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl MemoTableWithTypesMut<'_> {
    fn take_entries(&mut self) -> Option<Box<[MemoEntry]>> {
        let ptr = mem::replace(self.memos.entries.get_mut(), ptr::null_mut());
        if ptr.is_null() {
            return None;
        }
        // SAFETY: We detached the boxed slice under exclusive access. The type
        // table has the same length as when the allocation was published.
        Some(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, self.types.len())) })
    }

    #[inline]
    fn entries_mut(&mut self) -> &mut [MemoEntry] {
        let Some(ptr) = NonNull::new(*self.memos.entries.get_mut()) else {
            return &mut [];
        };
        // SAFETY: We have exclusive access to the fixed-length boxed slice.
        unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), self.types.len()) }
    }
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
    /// `self` must be the type table shared by the ingredient and the page
    /// that owns `memos`, as described in [`MemoTable`].
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
        let entries = self.entries().unwrap_or_else(|| self.initialize());
        let MemoEntry { atomic_memo } = &entries[index];

        // Verify that we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        let old_memo = atomic_memo.swap(MemoEntryType::to_dummy(memo).as_ptr(), Ordering::AcqRel);

        // SAFETY: We asserted that the type is correct above.
        NonNull::new(old_memo).map(|old_memo| unsafe { MemoEntryType::from_dummy(old_memo) })
    }

    /// Returns a pointer to the memo at the given index, if one has been inserted.
    #[inline]
    pub(crate) fn get<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<M>> {
        let index = memo_ingredient_index.as_usize();
        let MemoEntry { atomic_memo } = self.entries()?.get(index)?;
        let type_ = &self.types.types[index];

        // Verify that we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        NonNull::new(atomic_memo.load(Ordering::Acquire))
            // SAFETY: We asserted that the type is correct above.
            .map(|memo| unsafe { MemoEntryType::from_dummy(memo) })
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
        let MemoEntry { atomic_memo } = self.entries()?.get(index)?;
        let type_ = &self.types.types[index];

        let memo = NonNull::new(atomic_memo.load(Ordering::Acquire))?;

        // SAFETY: `insert` type-checks and release-publishes a pointer to the memo allocation's
        // base address, with spatial provenance covering the allocation, paired with `type_`; the
        // acquire load observes its initialization. The caller guarantees that the allocation
        // remains valid for `'a`.
        Some(unsafe { ErasedMemo::from_raw_parts(memo, type_.to_dyn_fn, type_.type_id) })
    }

    /// Returns memory usage information about the memos in this table.
    ///
    /// # Safety
    ///
    /// The caller must ensure exclusive access to the database's storage for the
    /// duration of this call, so memos cannot be replaced or freed during traversal.
    #[cfg(feature = "salsa_unstable")]
    pub(crate) unsafe fn memory_usage(&self) -> Vec<crate::database::MemoInfo> {
        let mut memory_usage = Vec::new();
        let entries = self.entries().unwrap_or_default();
        for (memo, type_) in entries.iter().zip(&self.types.types) {
            let Some(memo) = NonNull::new(memo.atomic_memo.load(Ordering::Acquire)) else {
                continue;
            };

            // SAFETY: The `TypeId` is asserted in `insert()`. The caller guarantees
            // exclusive database access, so the memo remains valid while we measure it.
            let dyn_memo: &dyn Memo = unsafe { (type_.to_dyn_fn)(memo).as_ref() };
            memory_usage.push(dyn_memo.memory_usage());
        }

        memory_usage
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
        mut self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(&mut M),
    ) {
        let index = memo_ingredient_index.as_usize();
        let types = self.types;
        let Some(MemoEntry { atomic_memo }) = self.entries_mut().get_mut(index) else {
            return;
        };
        let type_ = &types.types[index];

        // Verify that we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        let Some(memo) = NonNull::new(*atomic_memo.get_mut()) else {
            return;
        };

        // SAFETY: We asserted that the type is correct above.
        f(unsafe { MemoEntryType::from_dummy(memo).as_mut() });
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
        let Some(mut entries) = self.take_entries() else {
            return;
        };
        for (type_, memo) in self.types.types.iter().zip(entries.iter_mut()) {
            // SAFETY: The types match as per our constructor invariant.
            unsafe { memo.take(type_) };
        }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not call this function until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    pub(crate) unsafe fn take_memos(
        &mut self,
        mut f: impl FnMut(MemoIngredientIndex, Box<dyn Memo>),
    ) {
        let types = self.types;
        self.entries_mut()
            .iter_mut()
            .zip(&types.types)
            .enumerate()
            .filter_map(|(index, (memo, type_))| {
                // SAFETY: The types match as per our constructor invariant.
                let memo = unsafe { memo.take(type_)? };
                Some((MemoIngredientIndex::from_usize(index), memo))
            })
            .for_each(|(index, memo)| f(index, memo));

        // Keep the allocation attached until the last callback returns so the
        // caller's drop guard can clean up the remaining memos if one panics.
        drop(self.take_entries());
    }
}

/// This function is explicitly outlined to avoid debug machinery in the hot-path.
#[cold]
#[inline(never)]
fn type_assert_failed(memo_ingredient_index: MemoIngredientIndex) -> ! {
    panic!("inconsistent type-id for `{memo_ingredient_index:?}`")
}

impl MemoEntry {
    /// # Safety
    ///
    /// The type must match.
    #[inline]
    unsafe fn take(&mut self, type_: &MemoEntryType) -> Option<Box<dyn Memo>> {
        let memo = mem::replace(self.atomic_memo.get_mut(), ptr::null_mut());
        let memo = NonNull::new(memo)?;
        // SAFETY: Our preconditions.
        Some(unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) })
    }
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
