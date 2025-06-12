use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::mem;
use std::ptr::{self, NonNull};

use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::zalsa::MemoIngredientIndex;
use crate::zalsa::Zalsa;
use crate::DatabaseKeyIndex;

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
pub struct MemoTable {
    memos: Box<[MemoEntry]>,
}

impl MemoTable {
    /// Create a `MemoTable` with slots for memos from the provided `MemoTableTypes`.
    ///
    /// # Safety
    ///
    /// The created memo table must only be accessed with the same `MemoTableTypes`.
    pub unsafe fn new(types: &MemoTableTypes) -> Self {
        // Note that the safety invariant guarantees that any indices in-bounds for
        // this table are also in-bounds for its `MemoTableTypes`, as `MemoTableTypes`
        // is append-only.
        Self {
            memos: (0..types.len()).map(|_| MemoEntry::default()).collect(),
        }
    }

    /// Reset any memos in the table.
    ///
    /// Note that the memo entries should be freed manually before calling this function.
    pub fn reset(&mut self) {
        for memo in &mut self.memos {
            *memo = MemoEntry::default();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Represents a `Memo` that has one of two possible types.
///
/// # Safety
///
/// If the `value_type_id()` and the disambiguator match, the value must have the type of
/// the corresponding associated type.
pub unsafe trait AmbiguousMemo {
    /// The `TypeId` of the contained value.
    ///
    /// This can be shared to at most two `Memo` types, distinguished by `MFalse` and `MTrue`.
    /// The important property is that *both* can be stored in a slot. That is, the `MemoEntryType`
    /// only holds the `value_type_id()`, and the disambiguator (true or false) is stored
    /// in the `MemoEntry`.
    fn value_type_id() -> TypeId
    where
        Self: Sized;
    type MFalse: Memo;
    type MTrue: Memo;
}

pub trait Memo: Any + Send + Sync {
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

const DISAMBIGUATOR_MASK: usize = 0b1;

/// # Safety
///
/// `ptr` must stay non-null after removing the 0th bit.
#[inline]
unsafe fn unpack_memo_ptr(ptr: NonNull<DummyMemo>) -> (NonNull<DummyMemo>, bool) {
    let ptr = ptr.as_ptr();
    // SAFETY: Our precondition.
    let new_ptr =
        unsafe { NonNull::new_unchecked(ptr.map_addr(|addr| addr & !DISAMBIGUATOR_MASK)) };
    (new_ptr, ptr.addr() & DISAMBIGUATOR_MASK != 0)
}

#[inline]
fn pack_memo_ptr(ptr: NonNull<DummyMemo>, disambiguator: bool) -> NonNull<DummyMemo> {
    // SAFETY: We're ORing bits, it cannot make it null.
    unsafe {
        NonNull::new_unchecked(
            ptr.as_ptr()
                .map_addr(|addr| addr | usize::from(disambiguator)),
        )
    }
}

/// # Safety
///
/// `ptr` must stay non-null after removing the 0th bit. `value_type_id()` must be correct.
#[inline]
unsafe fn unpack_memo_ptr_typed<M: AmbiguousMemo>(
    ptr: NonNull<DummyMemo>,
) -> Either<NonNull<M::MFalse>, NonNull<M::MTrue>> {
    // SAFETY: Our precondition.
    let (new_ptr, disambiguator) = unsafe { unpack_memo_ptr(ptr) };
    match disambiguator {
        false => Either::Left(new_ptr.cast::<M::MFalse>()),
        true => Either::Right(new_ptr.cast::<M::MTrue>()),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MemoEntryType {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A type-coercion function for the erased memo type `M`, indexed by `type_id_disambiguator()`.
    to_dyn_fns: [fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>; 2],
}

impl MemoEntryType {
    #[inline]
    fn to_dyn_fn(&self, disambiguator: bool) -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
        self.to_dyn_fns[usize::from(disambiguator)]
    }

    const fn create_to_dyn_fn<M: Memo>() -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
        let f: fn(NonNull<M>) -> NonNull<dyn Memo> = |x| x;

        // SAFETY: `M: Sized` and `DummyMemo: Sized`, as such they are ABI compatible behind a
        // `NonNull` making it safe to do type erasure.
        unsafe {
            mem::transmute::<
                fn(NonNull<M>) -> NonNull<dyn Memo>,
                fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>,
            >(f)
        }
    }

    #[inline]
    pub fn of<M: AmbiguousMemo>() -> Self {
        const {
            assert!(
                align_of::<M::MFalse>() >= 2,
                "need enough space to encode the disambiguator"
            );
            assert!(
                align_of::<M::MTrue>() >= 2,
                "need enough space to encode the disambiguator"
            );
        };
        Self {
            type_id: M::value_type_id(),
            to_dyn_fns: [
                Self::create_to_dyn_fn::<M::MFalse>(),
                Self::create_to_dyn_fn::<M::MTrue>(),
            ],
        }
    }
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
#[derive(Debug)]
struct DummyMemo;

impl Memo for DummyMemo {
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
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the memo contents.
    #[inline]
    pub(crate) unsafe fn insert_false<M: AmbiguousMemo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M::MFalse>,
    ) -> Option<Either<NonNull<M::MFalse>, NonNull<M::MTrue>>> {
        let memo = pack_memo_ptr(memo.cast::<DummyMemo>(), false);
        // SAFETY: Our preconditions.
        unsafe { self.insert_impl::<M>(memo_ingredient_index, memo) }
    }
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the memo contents.
    #[inline]
    pub(crate) unsafe fn insert_true<M: AmbiguousMemo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M::MTrue>,
    ) -> Option<Either<NonNull<M::MFalse>, NonNull<M::MTrue>>> {
        let memo = pack_memo_ptr(memo.cast::<DummyMemo>(), true);
        // SAFETY: Our preconditions.
        unsafe { self.insert_impl::<M>(memo_ingredient_index, memo) }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the memo contents.
    #[inline]
    unsafe fn insert_impl<M: AmbiguousMemo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<DummyMemo>,
    ) -> Option<Either<NonNull<M::MFalse>, NonNull<M::MTrue>>> {
        let MemoEntry { atomic_memo } = self.memos.memos.get(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        // Verify that the we are casting to the correct type.
        if type_.type_id != M::value_type_id() {
            type_assert_failed(memo_ingredient_index);
        }

        let old_memo = atomic_memo.swap(memo.as_ptr(), Ordering::AcqRel);

        let old_memo = NonNull::new(old_memo);

        // SAFETY: `value_type_id()` check asserted above. The pointer points to a valid memo (otherwise
        // it'd be null) so not null.
        old_memo.map(|old_memo| unsafe { unpack_memo_ptr_typed::<M>(old_memo) })
    }

    /// Returns a pointer to the memo at the given index, if one has been inserted.
    #[inline]
    pub(crate) fn get<M: AmbiguousMemo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<Either<&'a M::MFalse, &'a M::MTrue>> {
        let MemoEntry { atomic_memo } = self.memos.memos.get(memo_ingredient_index.as_usize())?;

        // SAFETY: Any indices that are in-bounds for the `MemoTable` are also in-bounds for its
        // corresponding `MemoTableTypes`, by construction.
        let type_ = unsafe {
            self.types
                .types
                .get_unchecked(memo_ingredient_index.as_usize())
        };

        // Verify that the we are casting to the correct type.
        if type_.type_id != M::value_type_id() {
            type_assert_failed(memo_ingredient_index);
        }

        let memo = NonNull::new(atomic_memo.load(Ordering::Acquire));
        // SAFETY: `value_type_id()` check asserted above. The pointer points to a valid memo (otherwise
        // it'd be null) so not null.
        memo.map(|old_memo| unsafe {
            match unpack_memo_ptr_typed::<M>(old_memo) {
                Either::Left(it) => Either::Left(it.as_ref()),
                Either::Right(it) => Either::Right(it.as_ref()),
            }
        })
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn memory_usage(&self) -> Vec<crate::database::MemoInfo> {
        let mut memory_usage = Vec::new();
        for (index, memo) in self.memos.memos.iter().enumerate() {
            let Some(memo) = NonNull::new(memo.atomic_memo.load(Ordering::Acquire)) else {
                continue;
            };
            // SAFETY: There exists a memo so it's not null.
            let (memo, disambiguator) = unsafe { unpack_memo_ptr(memo) };

            let Some(type_) = self.types.types.get(index) else {
                continue;
            };

            // SAFETY: The `TypeId` is asserted in `insert()`.
            let dyn_memo: &dyn Memo = unsafe { type_.to_dyn_fn(disambiguator)(memo).as_ref() };
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
    pub(crate) fn map_memo<M: AmbiguousMemo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(Either<&mut M::MFalse, &mut M::MTrue>),
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
        if type_.type_id != M::value_type_id() {
            type_assert_failed(memo_ingredient_index);
        }

        let Some(memo) = NonNull::new(*atomic_memo.get_mut()) else {
            return;
        };

        // SAFETY: `value_type_id()` check asserted above. The pointer points to a valid memo (otherwise
        // it'd be null) so not null.
        f(unsafe {
            match unpack_memo_ptr_typed::<M>(memo) {
                Either::Left(mut it) => Either::Left(it.as_mut()),
                Either::Right(mut it) => Either::Right(it.as_mut()),
            }
        });
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
        for (type_, memo) in std::iter::zip(types, &mut self.memos.memos) {
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
        let memo = NonNull::new(mem::replace(self.atomic_memo.get_mut(), ptr::null_mut()))?;
        // SAFETY: We store an actual memo (otherwise `self.atomic_memo` would be null) of this type (our precondition).
        let (memo, disambiguator) = unsafe { unpack_memo_ptr(memo) };
        // SAFETY: Our preconditions.
        Some(unsafe { Box::from_raw(type_.to_dyn_fn(disambiguator)(memo).as_ptr()) })
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
