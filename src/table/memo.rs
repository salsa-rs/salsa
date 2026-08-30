use std::alloc::Layout;
use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::mem;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::function::ErasedMemo;
use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, IngredientIndex};

/// Adds the registered concrete memo type's vtable without dereferencing the pointer.
///
/// Dereferencing the result requires a live, aligned allocation of the same concrete type.
pub(crate) type ToDynMemo = fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>;

#[repr(C, align(16))]
struct MemoEntriesHeader {
    len: usize,
    entries: [MemoEntry; 0],
}

static EMPTY_MEMO_ENTRIES: MemoEntriesHeader = MemoEntriesHeader {
    len: 0,
    entries: [],
};

impl MemoEntriesHeader {
    /// # Safety
    ///
    /// The pointer point to a real value.
    #[inline]
    unsafe fn entries(ptr: *mut Self) -> *mut MemoEntry {
        // SAFETY: Our precondition.
        unsafe { (&raw mut (*ptr).entries).cast::<MemoEntry>() }
    }
}

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
pub struct MemoTable {
    /// # Invariant
    ///
    /// The last 4 bits (free due to the `repr(align(16))`) represent the generation, that is used
    /// to notice inserts when growing. We have 16 generations; if we need more, we allocate a new list (with generation 0).
    ///
    /// After clearing the last 4 LSBs, this is a pointer to a [`MemoEntriesHeader`]. [`MemoEntriesHeader::len`] holds
    /// the number of memos we have space to store: uninitialized memos have NULL. in [`MemoEntriesHeader::entries`]
    /// (and *not* necessarily at the end of the [`MemoEntriesHeader`] because of padding, we use the padding as well)
    /// there is space for `len` memos, then padding to be a multiple of the alignment.
    ///
    /// To avoid allocation for empty lists, they point at [`EMPTY_MEMO_ENTRIES`]. The generation is zero because no insertions
    /// are permitted. You can read the length and it'll be 0 as expected, but you're not allowed to deallocate this list.
    memos: AtomicPtr<MemoEntriesHeader>,
}

#[cfg(not(feature = "shuttle"))]
const _: [(); mem::size_of::<MemoTable>()] = [(); mem::size_of::<usize>()];

impl MemoTable {
    const GENERATION_MASK: usize = 16 - 1;
    const PTR_MASK: usize = !Self::GENERATION_MASK;

    /// Returns the layout for a list containing at least `len` memos, as well as the `len` of this list (which might
    /// be greater than the requested length if there is padding we can exploit).
    fn layout(len: usize) -> (usize, Layout) {
        let size = len
            .checked_mul(size_of::<MemoEntry>())
            .unwrap()
            .checked_add(mem::offset_of!(MemoEntriesHeader, entries))
            .unwrap();
        let align = std::cmp::max(align_of::<MemoEntriesHeader>(), align_of::<MemoEntry>());
        let layout = Layout::from_size_align(size, align).unwrap().pad_to_align();
        let len =
            (layout.size() - mem::offset_of!(MemoEntriesHeader, entries)) / size_of::<MemoEntry>();
        (len, layout)
    }

    /// Create a `MemoTable` with slots for memos from the provided `MemoTableTypes`.
    ///
    /// # Safety
    ///
    /// The created memo table must only be accessed with the same `MemoTableTypes`.
    pub unsafe fn new(_types: &MemoTableTypes) -> Self {
        // Note that the safety invariant guarantees that any indices in-bounds for
        // this table are also in-bounds for its `MemoTableTypes`, as `MemoTableTypes`
        // is append-only.
        Self {
            memos: AtomicPtr::new(ptr::from_ref(&EMPTY_MEMO_ENTRIES).cast_mut()),
        }
    }

    #[inline]
    fn get_mut(&mut self) -> &mut [MemoEntry] {
        let memos = self.memos.get_mut().map_addr(|addr| addr & Self::PTR_MASK);
        // SAFETY: The invariant of `self.memos`.
        unsafe { std::slice::from_raw_parts_mut(MemoEntriesHeader::entries(memos), (*memos).len) }
    }

    #[inline]
    fn get(&self) -> &[MemoEntry] {
        let memos = self
            .memos
            .load(Ordering::Acquire)
            .map_addr(|addr| addr & Self::PTR_MASK);
        // SAFETY: The invariant of `self.memos`.
        unsafe { std::slice::from_raw_parts(MemoEntriesHeader::entries(memos), (*memos).len) }
    }

    #[inline]
    fn insert(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        new_memo: *mut DummyMemo,
        zalsa: &Zalsa,
        optimal_init_len: usize,
    ) -> *mut DummyMemo {
        let expected_len = memo_ingredient_index.as_usize() + 1;
        let mut current = self.memos.load(Ordering::Acquire);
        // SAFETY: `self.memos`'s invariant.
        let memo_to_write = |memos| unsafe {
            &(*MemoEntriesHeader::entries(memos).add(memo_ingredient_index.as_usize())).atomic_memo
        };

        let mut memos = current.map_addr(|addr| addr & Self::PTR_MASK);
        let mut generation = current.addr() & Self::GENERATION_MASK;
        // SAFETY: `self.memos`'s invariant.
        let mut current_len = unsafe { (*memos).len };
        let old_memo = if expected_len > current_len {
            ptr::null_mut()
        } else {
            // We rely on the synchronization between queries, so we only need to load the previous memo once.
            // Even if the synchronization breaks, we might have memory leaks but not UB.
            memo_to_write(memos).swap(new_memo, Ordering::AcqRel)
        };

        loop {
            if expected_len > current_len || generation >= Self::GENERATION_MASK {
                // We have no space for the new memo or no space to increase the generation.
                self.refresh_allocation(
                    memo_ingredient_index,
                    new_memo,
                    current,
                    zalsa,
                    optimal_init_len,
                );
                return old_memo;
            }

            // We can just increment the pointer because we know the generation won't overflow.
            match self.memos.compare_exchange_weak(
                current,
                current.map_addr(|addr| addr + 1),
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // We successfully updated the pointer; it could not have been replaced by another `refresh_allocation()`
                    // between the write and the `compare_exchange_weak()` because we loaded it before the write, and there is
                    // a `refresh_allocation()` in progress it'll fail because the pointer has changed.
                    return old_memo;
                }
                Err(cur) => current = cur,
            }

            memos = current.map_addr(|addr| addr & Self::PTR_MASK);
            generation = current.addr() & Self::GENERATION_MASK;
            // SAFETY: `self.memos`'s invariant.
            current_len = unsafe { (*memos).len };

            memo_to_write(memos).store(new_memo, Ordering::Release);
        }
    }

    #[cold]
    fn refresh_allocation(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        new_memo: *mut DummyMemo,
        mut current: *mut MemoEntriesHeader,
        zalsa: &Zalsa,
        optimal_init_len: usize,
    ) {
        let expected_len = std::cmp::max(memo_ingredient_index.as_usize() + 1, optimal_init_len);
        let mut memos = current.map_addr(|addr| addr & Self::PTR_MASK);
        // SAFETY: `self.memos`'s invariant.
        let mut current_len = unsafe { (*memos).len };
        let expected_len = std::cmp::max(current_len, expected_len);

        let (expected_len, layout) = Self::layout(expected_len);
        // SAFETY: `layout` is not zero-sized because it contains at least the header.
        let new = unsafe { std::alloc::alloc(layout).cast::<MemoEntriesHeader>() };
        if new.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        // SAFETY: Those are in bounds for the new allocation.
        unsafe {
            ptr::write(&raw mut (*new).len, expected_len);
            // Null-initialize excess memos. This might be overridden partially later if `self.memos` is replaced with a list
            // that is not big enough for us but is bigger than the current (if it's big enough for us, we'll just discard
            // this allocation).
            ptr::write_bytes::<MemoEntry>(
                MemoEntriesHeader::entries(new).add(current_len),
                0,
                expected_len - current_len,
            );
        }

        loop {
            // SAFETY: `self.memos`'s invariant.
            let current_memos = unsafe {
                std::slice::from_raw_parts(MemoEntriesHeader::entries(memos), current_len)
            };
            // SAFETY: Those are in bounds for the new allocation.
            unsafe {
                for (index, memo) in current_memos.iter().enumerate() {
                    // Note: we must use an atomic load here (but not store, since `new` is unique to us).
                    ptr::write::<MemoEntry>(
                        MemoEntriesHeader::entries(new).add(index),
                        MemoEntry {
                            atomic_memo: AtomicPtr::new(memo.atomic_memo.load(Ordering::Acquire)),
                        },
                    );
                }
                ptr::write::<MemoEntry>(
                    MemoEntriesHeader::entries(new).add(memo_ingredient_index.as_usize()),
                    MemoEntry {
                        atomic_memo: AtomicPtr::new(new_memo),
                    },
                );
            }

            match self
                .memos
                .compare_exchange(current, new, Ordering::Release, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(cur) => {
                    current = cur;
                    memos = current.map_addr(|addr| addr & Self::PTR_MASK);
                    // SAFETY: `self.memos`'s invariant.
                    current_len = unsafe { (*memos).len };

                    if current_len >= expected_len {
                        // Someone has raced with us and already allocated enough space. Call `insert()` so it'll handle incrementing
                        // the generation etc.

                        // SAFETY: We allocated this just above.
                        unsafe {
                            std::alloc::dealloc(new.cast::<u8>(), layout);
                        }

                        self.insert(memo_ingredient_index, new_memo, zalsa, optimal_init_len);
                        return;
                    }
                }
            }
        }

        if memos.cast_const() != &EMPTY_MEMO_ENTRIES {
            zalsa.push_deleted_memo_table(MemoTable {
                memos: AtomicPtr::new(memos),
            });
        }
    }

    /// Reset any memos in the table.
    ///
    /// Note that the memo entries should be freed manually before calling this function.
    pub fn reset(&mut self) {
        for memo in self.get_mut() {
            *memo = MemoEntry::default();
        }
    }

    pub(crate) fn update_frequency_stats(
        &self,
        memo_ingredient_indices: &[IngredientIndex],
        freq_stats: &mut FxHashMap<IngredientIndex, u32>,
        total_count: &mut u32,
    ) {
        let memos = self.get();
        if !memos.is_empty() {
            *total_count += 1;
        }
        for (&memo_index, entry) in std::iter::zip(memo_ingredient_indices, memos) {
            if !entry.atomic_memo.load(Ordering::Relaxed).is_null() {
                *freq_stats.entry(memo_index).or_insert(0) += 1;
            }
        }
    }
}

impl Drop for MemoTable {
    fn drop(&mut self) {
        let memos = *self.memos.get_mut();
        if memos.cast_const() != &EMPTY_MEMO_ENTRIES {
            // SAFETY: We compute the layout backwards from the length. The allocation is ours it's not empty.
            unsafe {
                let memos = memos.map_addr(|addr| addr & Self::PTR_MASK);
                let len = (*memos).len;
                let (layout_len, layout) = Self::layout(len);
                debug_assert_eq!(len, layout_len);
                std::alloc::dealloc(memos.cast::<u8>(), layout);
            }
        }
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

pub struct MemoTableTypes {
    types: Vec<MemoEntryType>,
    optimal_table_init_len: usize,
}

impl MemoTableTypes {
    pub(crate) fn new(zalsa: &Zalsa, ingredient_debug_name: &str) -> Arc<Self> {
        Arc::new(Self {
            types: Vec::new(),
            optimal_table_init_len: zalsa.optimal_memo_table_init_len(ingredient_debug_name),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn empty() -> Self {
        Self {
            types: Vec::new(),
            optimal_table_init_len: 0,
        }
    }

    pub(crate) fn set(
        &mut self,
        memo_ingredient_index: MemoIngredientIndex,
        memo_type: MemoEntryType,
    ) {
        if self.types.len() <= memo_ingredient_index.as_usize() {
            self.types
                .resize_with(memo_ingredient_index.as_usize() + 1, || {
                    MemoEntryType::of::<DummyMemo>()
                });
        }
        self.types[memo_ingredient_index.as_usize()] = memo_type;
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
        zalsa: &Zalsa,
    ) -> Option<NonNull<M>> {
        let type_ = &self.types.types[memo_ingredient_index.as_usize()];

        // Verify that the we are casting to the correct type.
        if type_.type_id != TypeId::of::<M>() {
            type_assert_failed(memo_ingredient_index);
        }

        let old_memo = self.memos.insert(
            memo_ingredient_index,
            MemoEntryType::to_dummy(memo).as_ptr(),
            zalsa,
            self.types.optimal_table_init_len,
        );

        // SAFETY: We asserted that the type is correct above.
        NonNull::new(old_memo).map(|old_memo| unsafe { MemoEntryType::from_dummy(old_memo) })
    }

    /// Returns a pointer to the memo at the given index, if one has been inserted.
    #[inline]
    pub(crate) fn get<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<M>> {
        let MemoEntry { atomic_memo } = self.memos.get().get(memo_ingredient_index.as_usize())?;

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
    /// shared access for `'a`.
    #[inline]
    unsafe fn get_erased(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ErasedMemo<'a>> {
        let MemoEntry { atomic_memo } = self.memos.get().get(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        let memo = NonNull::new(atomic_memo.load(Ordering::Acquire))?;

        // SAFETY: `insert` type-checks and release-publishes a pointer to the memo allocation's
        // base address, with spatial provenance covering the allocation, paired with `type_`; the
        // acquire load observes its initialization. The caller guarantees that the allocation
        // remains valid for `'a`.
        Some(unsafe { ErasedMemo::from_raw_parts(memo, type_.to_dyn_fn, type_.type_id) })
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn memory_usage(&self) -> Vec<crate::database::MemoInfo> {
        let mut memory_usage = Vec::new();
        for (index, memo) in self.memos.get().iter().enumerate() {
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
        let Some(MemoEntry { atomic_memo }) = self
            .memos
            .get_mut()
            .get_mut(memo_ingredient_index.as_usize())
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
        for (type_, memo) in std::iter::zip(types, self.memos.get_mut()) {
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
            .get_mut()
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
