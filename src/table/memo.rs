use std::{
    any::{Any, TypeId},
    fmt::Debug,
    mem,
    ptr::{self, NonNull},
};

use thin_vec::ThinVec;

use crate::loom::sync::atomic::{AtomicPtr, Ordering};
use crate::loom::sync::{AtomicMut, OnceLock, RwLock};
use crate::{zalsa::MemoIngredientIndex, zalsa_local::QueryOrigin};

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<ThinVec<MemoEntry>>,
}

impl MemoTable {
    pub(crate) fn clear(&self) {
        self.memos.write().clear();
    }
}

pub trait Memo: Any + Send + Sync {
    /// Returns the `origin` of this memo
    fn origin(&self) -> &QueryOrigin;
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
#[derive(Default)]
struct MemoEntry {
    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

pub struct MemoEntryType {
    data: OnceLock<MemoEntryTypeData>,
}

#[derive(Clone, Copy)]
struct MemoEntryTypeData {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A type-coercion function for the erased memo type `M`
    to_dyn_fn: fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>,
}

impl MemoEntryType {
    fn to_dummy<M: Memo>(memo: NonNull<M>) -> NonNull<DummyMemo> {
        memo.cast()
    }

    unsafe fn from_dummy<M: Memo>(memo: NonNull<DummyMemo>) -> NonNull<M> {
        memo.cast()
    }

    const fn to_dyn_fn<M: Memo>() -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
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
    pub fn of<M: Memo>() -> Self {
        Self {
            data: OnceLock::from(MemoEntryTypeData {
                type_id: TypeId::of::<M>(),
                to_dyn_fn: Self::to_dyn_fn::<M>(),
            }),
        }
    }

    #[inline]
    fn load(&self) -> Option<&MemoEntryTypeData> {
        self.data.get()
    }
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
#[derive(Debug)]
struct DummyMemo {}

impl Memo for DummyMemo {
    fn origin(&self) -> &QueryOrigin {
        unreachable!("should not get here")
    }
}

#[derive(Default)]
pub struct MemoTableTypes {
    types: boxcar::Vec<MemoEntryType>,
}

impl MemoTableTypes {
    pub(crate) fn set(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo_type: &MemoEntryType,
    ) {
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        while memo_ingredient_index >= self.types.count() {
            self.types.push(MemoEntryType {
                data: OnceLock::new(),
            });
        }
        let memo_entry_type = self.types.get(memo_ingredient_index).unwrap();
        memo_entry_type
            .data
            .set(
                *memo_type
                    .data
                    .get()
                    .expect("cannot provide an empty `MemoEntryType` for `MemoEntryType::set()`"),
            )
            .ok()
            .expect("memo type should only be set once");
    }

    /// # Safety
    ///
    /// The types table must be the correct one of `memos`.
    #[inline]
    pub(super) unsafe fn attach_memos<'a>(
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

pub(crate) struct MemoTableWithTypes<'a> {
    types: &'a MemoTableTypes,
    memos: &'a MemoTable,
}

impl MemoTableWithTypes<'_> {
    pub(crate) fn insert<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        // The type must already exist, we insert it when creating the memo ingredient.
        assert_eq!(
            self.types
                .types
                .get(memo_ingredient_index.as_usize())
                .and_then(MemoEntryType::load)?
                .type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );

        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        if let Some(MemoEntry { atomic_memo }) = self
            .memos
            .memos
            .read()
            .get(memo_ingredient_index.as_usize())
        {
            let old_memo =
                atomic_memo.swap(MemoEntryType::to_dummy(memo).as_ptr(), Ordering::AcqRel);

            let old_memo = NonNull::new(old_memo);

            // SAFETY: `type_id` check asserted above
            return old_memo.map(|old_memo| unsafe { MemoEntryType::from_dummy(old_memo) });
        }

        // Otherwise we need the write lock.
        self.insert_cold(memo_ingredient_index, memo)
    }

    #[cold]
    fn insert_cold<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        let mut memos = self.memos.memos.write();

        // Grow the table if needed.
        if memos.len() <= memo_ingredient_index {
            let additional_len = memo_ingredient_index - memos.len() + 1;
            memos.reserve(additional_len);
            while memos.len() <= memo_ingredient_index {
                memos.push(MemoEntry::default());
            }
        }

        let memo_entry = &mut memos[memo_ingredient_index].atomic_memo;
        let old_entry = memo_entry.read_mut();
        memo_entry.write_mut(MemoEntryType::to_dummy(memo).as_ptr());

        // SAFETY: The `TypeId` is asserted in `insert()`.
        NonNull::new(old_entry).map(|memo| unsafe { MemoEntryType::from_dummy(memo) })
    }

    #[inline]
    pub(crate) fn get<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<M>> {
        let read = self.memos.memos.read();
        let memo = read.get(memo_ingredient_index.as_usize())?;
        let type_ = self
            .types
            .types
            .get(memo_ingredient_index.as_usize())
            .and_then(MemoEntryType::load)?;
        assert_eq!(
            type_.type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );
        let memo = NonNull::new(memo.atomic_memo.load(Ordering::Acquire))?;
        // SAFETY: `type_id` check asserted above
        Some(unsafe { MemoEntryType::from_dummy(memo) })
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
        let Some(type_) = self
            .types
            .types
            .get(memo_ingredient_index.as_usize())
            .and_then(MemoEntryType::load)
        else {
            return;
        };
        assert_eq!(
            type_.type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );

        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        let memos = self.memos.memos.get_mut();
        let Some(MemoEntry { atomic_memo }) = memos.get_mut(memo_ingredient_index.as_usize())
        else {
            return;
        };
        let Some(memo) = NonNull::new(atomic_memo.read_mut()) else {
            return;
        };

        // SAFETY: `type_id` check asserted above
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
        for ((_, type_), memo) in std::iter::zip(types, self.memos.memos.get_mut()) {
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
        let memos = self.memos.memos.get_mut();
        memos
            .iter_mut()
            .zip(self.types.types.iter())
            .enumerate()
            .filter_map(|(index, (memo, (_, type_)))| {
                // SAFETY: The types match as per our constructor invariant.
                let memo = unsafe { memo.take(type_)? };
                Some((MemoIngredientIndex::from_usize(index), memo))
            })
            .for_each(|(index, memo)| f(index, memo));
    }
}

impl MemoEntry {
    /// # Safety
    ///
    /// The type must match.
    #[inline]
    unsafe fn take(&mut self, type_: &MemoEntryType) -> Option<Box<dyn Memo>> {
        let memo = NonNull::new(self.atomic_memo.read_mut());
        self.atomic_memo.write_mut(ptr::null_mut());
        let type_ = type_.load()?;
        // SAFETY: Our preconditions.
        Some(unsafe { Box::from_raw((type_.to_dyn_fn)(memo?).as_ptr()) })
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
