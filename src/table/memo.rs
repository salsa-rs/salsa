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
pub struct MemoTable {
    memos: LazyMemoEntries,
}

#[cfg(not(feature = "shuttle"))]
const _: [(); mem::size_of::<MemoTable>()] = [(); 2 * mem::size_of::<usize>()];

impl MemoTable {
    /// Create a `MemoTable` that allocates slots on the first memo insertion.
    ///
    /// # Safety
    ///
    /// The created memo table must only be accessed with the same `MemoTableTypes`.
    pub unsafe fn new(types: &MemoTableTypes) -> Self {
        // Note that the safety invariant guarantees that any indices in-bounds for
        // this table are also in-bounds for its `MemoTableTypes`, as `MemoTableTypes`
        // is append-only.
        Self {
            memos: LazyMemoEntries::new(types.len()),
        }
    }

    /// Reset any memos in the table.
    ///
    /// Note that the memo entries should be freed manually before calling this function.
    pub fn reset(&mut self) {
        self.memos.clear();
    }
}

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
/// when freeing `MemoEntryData` values to transmute things back. See the `Drop` impl for
/// [`MemoEntry`][] for details.
#[derive(Default, Debug)]
struct MemoEntry {
    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

/// Lazily allocated, fixed-length memo entries.
///
/// The pointer and length have the same inline layout as an eager `Box<[MemoEntry]>`, but a null
/// pointer represents an allocation that has not been created yet.
struct LazyMemoEntries {
    ptr: AtomicPtr<MemoEntry>,
    len: usize,
}

impl LazyMemoEntries {
    fn new(len: usize) -> Self {
        Self {
            ptr: AtomicPtr::new(ptr::null_mut()),
            len,
        }
    }

    #[inline]
    fn get(&self, index: usize) -> Option<&MemoEntry> {
        self.as_slice()?.get(index)
    }

    #[inline]
    fn get_or_init(&self, index: usize) -> Option<&MemoEntry> {
        if index >= self.len {
            return None;
        }

        let memos = self.as_slice().unwrap_or_else(|| self.initialize());
        Some(&memos[index])
    }

    #[inline]
    fn get_mut(&mut self, index: usize) -> Option<&mut MemoEntry> {
        self.as_mut_slice()?.get_mut(index)
    }

    fn iter(&self) -> std::slice::Iter<'_, MemoEntry> {
        self.as_slice().unwrap_or_default().iter()
    }

    fn iter_mut(&mut self) -> std::slice::IterMut<'_, MemoEntry> {
        self.as_mut_slice().unwrap_or_default().iter_mut()
    }

    #[inline]
    fn as_slice(&self) -> Option<&[MemoEntry]> {
        let ptr = NonNull::new(self.ptr.load(Ordering::Acquire))?;

        // The acquire load synchronizes with the release operation that published the pointer,
        // ensuring that the memo entries are initialized before we create references to them.
        //
        // SAFETY: A non-null pointer comes from a boxed slice of length `self.len`. The allocation
        // cannot be freed while `self` is shared.
        Some(unsafe { std::slice::from_raw_parts(ptr.as_ptr(), self.len) })
    }

    #[inline]
    fn as_mut_slice(&mut self) -> Option<&mut [MemoEntry]> {
        let ptr = NonNull::new(*self.ptr.get_mut())?;

        // SAFETY: A non-null pointer comes from a boxed slice of length `self.len`, and exclusive
        // access guarantees that no other references exist.
        Some(unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), self.len) })
    }

    #[cold]
    fn initialize(&self) -> &[MemoEntry] {
        let new_memos: Box<[MemoEntry]> = (0..self.len).map(|_| MemoEntry::default()).collect();
        let new_memos = Box::into_raw(new_memos);
        let new_memos_ptr = new_memos.cast::<MemoEntry>();

        // Release publishes the initialized memo entries. If another thread won the race, acquire
        // synchronizes with its release operation before we create references to its allocation.
        let ptr = match self.ptr.compare_exchange(
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
        // another thread. Both allocations have length `self.len` and cannot be freed while `self`
        // is shared.
        unsafe { std::slice::from_raw_parts(ptr, self.len) }
    }

    fn clear(&mut self) {
        let ptr = mem::replace(self.ptr.get_mut(), ptr::null_mut());
        if ptr.is_null() {
            return;
        }

        // SAFETY: `ptr` came from a boxed slice of length `self.len`, and exclusive access
        // guarantees that no references to the allocation remain.
        unsafe { drop(Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, self.len))) };
    }
}

impl Drop for LazyMemoEntries {
    fn drop(&mut self) {
        self.clear();
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
    /// The types table must be the correct one of `memos`.
    #[inline]
    pub(crate) unsafe fn attach_memos<'a>(
        &'a self,
        memos: &'a MemoTable,
    ) -> MemoTableWithTypes<'a> {
        MemoTableWithTypes { types: self, memos }
    }

    /// # Safety
    ///
    /// The types table must be the correct one of `memos`.
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

impl<'a> MemoTableWithTypes<'a> {
    pub(crate) fn insert<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        let MemoEntry { atomic_memo } = self
            .memos
            .memos
            .get_or_init(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        // Verify that the we are casting to the correct type.
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
        self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<M>> {
        let MemoEntry { atomic_memo } = self.memos.memos.get(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        // Verify that the we are casting to the correct type.
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
    /// shared access for `'a`, even if another thread replaces the entry.
    #[inline]
    pub(crate) unsafe fn get_erased(
        self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ErasedMemo<'a>> {
        let MemoEntry { atomic_memo } = self.memos.memos.get(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        let memo = NonNull::new(atomic_memo.load(Ordering::Acquire))?;

        // SAFETY: `insert` type-checks and release-publishes a complete-allocation pointer paired
        // with `type_`; the acquire load observes its initialization. The caller guarantees that
        // the allocation remains valid for `'a`.
        Some(unsafe { ErasedMemo::from_raw_parts(memo, type_.to_dyn_fn, type_.type_id) })
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn memory_usage(&self) -> Vec<crate::database::MemoInfo> {
        let mut memory_usage = Vec::new();
        for (index, memo) in self.memos.memos.iter().enumerate() {
            let Some(memo) = NonNull::new(memo.atomic_memo.load(Ordering::Acquire)) else {
                continue;
            };

            let Some(type_) = self.types.types.get(index) else {
                continue;
            };

            // SAFETY: The `TypeId` is asserted in `insert()`.
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
        self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(&mut M),
    ) {
        let Some(MemoEntry { atomic_memo }) =
            self.memos.memos.get_mut(memo_ingredient_index.as_usize())
        else {
            return;
        };

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        // Verify that the we are casting to the correct type.
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
        let types = self.types.types.iter();
        for (type_, memo) in std::iter::zip(types, self.memos.memos.iter_mut()) {
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
        self.memos
            .memos
            .iter_mut()
            .zip(self.types.types.iter())
            .enumerate()
            .filter_map(|(index, (memo, type_))| {
                // SAFETY: The types match as per our constructor invariant.
                let memo = unsafe { memo.take(type_)? };
                Some((MemoIngredientIndex::from_usize(index), memo))
            })
            .for_each(|(index, memo)| f(index, memo));
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
