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
    /// Null, a tagged memo pointer, or a pointer to the full array.
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

// Bit 0 distinguishes an inline memo from an aligned array pointer. The
// next seven bits hold the index. Shifting the address leaves 56 bits for
// it on 64-bit targets. Only encode an address if the shift is lossless.
const TAG_BITS: u32 = 8;
const MAX_INLINE_INDEX: usize = (1 << (TAG_BITS - 1)) - 1;

#[inline]
fn encode_inline(index: usize, memo: NonNull<DummyMemo>) -> Option<*mut MemoEntry> {
    const { assert!(mem::align_of::<MemoEntry>() >= 2) };
    let ptr = memo.as_ptr();
    if index > MAX_INLINE_INDEX || ptr.addr() > usize::MAX >> TAG_BITS {
        return None;
    }
    Some(
        ptr.map_addr(|addr| (addr << TAG_BITS) | (index << 1) | 1)
            .cast(),
    )
}

#[inline]
fn decode_inline(state: *mut MemoEntry) -> Option<(usize, *mut DummyMemo)> {
    if state.addr() & 1 == 0 {
        return None;
    }
    let index = (state.addr() >> 1) & MAX_INLINE_INDEX;
    // map_addr preserves the memo allocation's provenance through both shifts.
    let memo = state.map_addr(|addr| addr >> TAG_BITS).cast();
    Some((index, memo))
}

impl MemoTableWithTypes<'_> {
    #[inline]
    fn load(&self, index: usize) -> Option<NonNull<DummyMemo>> {
        let state = NonNull::new(self.memos.entries.load(Ordering::Acquire))?.as_ptr();
        if let Some((first, memo)) = decode_inline(state) {
            return if first == index {
                NonNull::new(memo)
            } else {
                None
            };
        }
        // SAFETY: An untagged, non-null pointer is a release-published array
        // of the registered length. It stays allocated until exclusive access.
        let entries = unsafe { std::slice::from_raw_parts(state, self.types.len()) };
        NonNull::new(entries.get(index)?.atomic_memo.load(Ordering::Acquire))
    }

    fn store(&self, index: usize, memo: NonNull<DummyMemo>) -> *mut DummyMemo {
        let word = &self.memos.entries;
        let mut state = word.load(Ordering::Acquire);
        loop {
            let inline = decode_inline(state);
            if inline.is_none() && !state.is_null() {
                // SAFETY: The published array has one entry per registered
                // type and stays in place under shared access.
                let entries = unsafe { std::slice::from_raw_parts(state, self.types.len()) };
                return entries[index]
                    .atomic_memo
                    .swap(memo.as_ptr(), Ordering::AcqRel);
            }

            if inline.is_none_or(|(first, _)| first == index)
                && let Some(new) = encode_inline(index, memo)
            {
                match word.compare_exchange(state, new, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return inline.map_or(ptr::null_mut(), |(_, old)| old),
                    Err(current) => state = current,
                }
            } else {
                match self.promote(state, index, memo) {
                    Ok(old) => return old,
                    Err(current) => state = current,
                }
            }
        }
    }

    #[cold]
    fn promote(
        &self,
        state: *mut MemoEntry,
        index: usize,
        memo: NonNull<DummyMemo>,
    ) -> Result<*mut DummyMemo, *mut MemoEntry> {
        let len = self.types.len();
        // Collecting defaults can compile to a zeroed allocation, which is
        // more expensive for small memo tables with jemalloc.
        let mut entries = Vec::with_capacity(len);
        entries.resize_with(len, MemoEntry::default);
        if let Some((first, previous)) = decode_inline(state) {
            *entries[first].atomic_memo.get_mut() = previous;
        }
        // A replacement at the same index can also require promotion if its
        // address no longer fits. Return the old pointer to the caller.
        let old = mem::replace(entries[index].atomic_memo.get_mut(), memo.as_ptr());
        let array = Box::into_raw(entries.into_boxed_slice());

        // Publish both entries together. If another writer replaced the inline
        // memo or published an array, retry with its state. Replaced memos stay
        // alive until the end of the revision, so their addresses can't be reused.
        match self.memos.entries.compare_exchange(
            state,
            array.cast(),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(old),
            Err(current) => {
                // SAFETY: This array was never published. Dropping its atomic
                // pointers frees the array without dropping either boxed memo.
                unsafe { drop(Box::from_raw(array)) };
                Err(current)
            }
        }
    }
}

impl MemoTableWithTypesMut<'_> {
    /// # Safety
    /// The caller must ensure there are no outstanding references to the memo.
    unsafe fn take_inline(&mut self) -> Option<(usize, Box<dyn Memo>)> {
        let state = self.memos.entries.get_mut();
        let (index, memo) = decode_inline(*state)?;
        *state = ptr::null_mut();
        let memo = NonNull::new(memo).unwrap();
        // SAFETY: We detached the original boxed memo under exclusive access.
        // Insertion checked its type against the descriptor at this index.
        let boxed = unsafe { Box::from_raw((self.types.types[index].to_dyn_fn)(memo).as_ptr()) };
        Some((index, boxed))
    }

    fn take_entries(&mut self) -> Option<Box<[MemoEntry]>> {
        let ptr = mem::replace(self.memos.entries.get_mut(), ptr::null_mut());
        assert!(decode_inline(ptr).is_none());
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
        assert!(decode_inline(ptr.as_ptr()).is_none());
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

        let old_memo = self.store(index, MemoEntryType::to_dummy(memo));

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
        let state = self.memos.entries.load(Ordering::Acquire);
        if state.is_null() {
            return (0, Vec::new());
        }
        let measure = |memo: NonNull<DummyMemo>, type_: &MemoEntryType| {
            // SAFETY: Insertion checked the type. Exclusive database access
            // keeps the memo alive while measuring it.
            unsafe { (type_.to_dyn_fn)(memo).as_ref().memory_usage() }
        };
        if let Some((index, memo)) = decode_inline(state) {
            let info = measure(NonNull::new(memo).unwrap(), &self.types.types[index]);
            return (0, vec![info]);
        }
        // SAFETY: This is the array published by promotion, with one entry per type.
        let entries = unsafe { std::slice::from_raw_parts(state, self.types.len()) };
        let memos = entries
            .iter()
            .zip(&self.types.types)
            .filter_map(|(entry, type_)| {
                let memo = NonNull::new(entry.atomic_memo.load(Ordering::Acquire))?;
                Some(measure(memo, type_))
            })
            .collect();
        (mem::size_of_val(entries), memos)
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
        if unsafe { self.take_inline() }.is_some() {
            return;
        }
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
        // SAFETY: The caller guarantees that no memo is still borrowed.
        if let Some((index, memo)) = unsafe { self.take_inline() } {
            f(MemoIngredientIndex::from_usize(index), memo);
            return;
        }
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

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;

    struct TestMemo(usize);

    impl Memo for TestMemo {
        fn has_value(&self) -> bool {
            true
        }

        fn remove_outputs(&self, _: &Zalsa, _: DatabaseKeyIndex) {}

        #[cfg(feature = "salsa_unstable")]
        fn memory_usage(&self) -> crate::database::MemoInfo {
            unreachable!()
        }
    }

    fn types(len: usize) -> MemoTableTypes {
        MemoTableTypes {
            types: (0..len).map(|_| MemoEntryType::of::<TestMemo>()).collect(),
        }
    }

    fn memo(value: usize) -> NonNull<TestMemo> {
        NonNull::new(Box::into_raw(Box::new(TestMemo(value)))).unwrap()
    }

    #[test]
    fn inline_encoding_is_lossless_or_falls_back() {
        let max_address = usize::MAX >> TAG_BITS;
        let pointer = |address| NonNull::new(ptr::without_provenance_mut(address)).unwrap();
        let word = encode_inline(127, pointer(max_address)).unwrap();
        assert_eq!(
            decode_inline(word),
            Some((127, pointer(max_address).as_ptr()))
        );
        assert!(encode_inline(128, pointer(1)).is_none());
        assert!(encode_inline(0, pointer(max_address + 1)).is_none());
    }

    #[test]
    fn promotion_preserves_memo_addresses_and_iteration_order() {
        let types = types(129);
        let mut memos = MemoTable::default();
        let index = MemoIngredientIndex::from_usize;
        // Exercise both an inline first insertion and a first index too large
        // to encode. Reuse the same table after draining it.
        for first in [14, 128] {
            // SAFETY: This test uses the same type list for every access to `memos`.
            let table = || unsafe { types.attach_memos(&memos) };
            let original = memo(1);
            assert!(table().insert(index(first), original).is_none());
            let state = memos.entries.load(Ordering::Acquire);
            if let Some(encoded) = encode_inline(first, original.cast()) {
                assert_eq!(state, encoded);
            } else {
                assert!(decode_inline(state).is_none());
            }
            assert!(table().get::<TestMemo>(index(0)).is_none());
            assert!(table().insert(index(0), memo(2)).is_none());
            assert_eq!(table().get(index(first)), Some(original));
            let mut order = Vec::new();
            // SAFETY: This table has no outstanding readers.
            unsafe {
                types
                    .attach_memos_mut(&mut memos)
                    .take_memos(|i, _| order.push(i.as_usize()))
            };
            assert_eq!(order, [0, first]);
            assert!(memos.entries.get_mut().is_null());
        }
    }

    #[test]
    fn concurrent_replacement_and_promotion_keep_all_memos() {
        let types = types(3);
        let mut memos = MemoTable::default();
        let index = MemoIngredientIndex::from_usize;
        // SAFETY: All threads share the same immutable type list for `memos`.
        let table = || unsafe { types.attach_memos(&memos) };
        let first = memo(0);
        assert!(table().insert(index(1), first).is_none());
        let start = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            for i in [0, 2] {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    assert!(table().insert(index(i), memo(i)).is_none());
                });
            }
            start.wait();
            assert_eq!(table().insert(index(1), memo(1)), Some(first));
        });
        for i in 0..3 {
            let memo = table().get::<TestMemo>(index(i)).unwrap();
            // SAFETY: The table owns these allocations and all writers have joined.
            assert_eq!(unsafe { memo.as_ref() }.0, i);
        }
        // SAFETY: Replacement returned ownership of the first allocation.
        // All readers and writers have finished using the table.
        unsafe {
            drop(Box::from_raw(first.as_ptr()));
            types.attach_memos_mut(&mut memos).drop();
        }
    }
}
