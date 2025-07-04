use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::mem;
use std::ptr::{self, NonNull};

use portable_atomic::hint::spin_loop;
use thin_vec::ThinVec;

use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::sync::{OnceLock, RwLock};
use crate::{zalsa::MemoIngredientIndex, zalsa_local::QueryOriginRef};

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<ThinVec<MemoEntry>>,
}

pub trait Memo: Any + Send + Sync {
    /// Returns the `origin` of this memo
    fn origin(&self) -> QueryOriginRef<'_>;

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
#[derive(Default)]
struct MemoEntry {
    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

#[derive(Default)]
pub struct MemoEntryType {
    data: OnceLock<MemoEntryTypeData>,
}

#[derive(Clone, Copy, Debug)]
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
struct DummyMemo;

impl Memo for DummyMemo {
    fn origin(&self) -> QueryOriginRef<'_> {
        unreachable!("should not get here")
    }

    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self) -> crate::database::MemoInfo {
        crate::database::MemoInfo {
            debug_name: "dummy",
            output: crate::database::SlotInfo {
                debug_name: "dummy",
                size_of_metadata: 0,
                size_of_fields: 0,
                memos: Vec::new(),
            },
        }
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

        // Try to create our entry if it has not already been created.
        if memo_ingredient_index >= self.types.count() {
            while self.types.push(MemoEntryType::default()) < memo_ingredient_index {}
        }

        loop {
            let Some(memo_entry_type) = self.types.get(memo_ingredient_index) else {
                // It's possible that someone else began pushing to our index but has not
                // completed the entry's initialization yet, as `boxcar` is lock-free. This
                // is extremely unlikely given initialization is just a handful of instructions.
                // Additionally, this function is generally only called on startup, so we can
                // just spin here.
                spin_loop();
                continue;
            };

            memo_entry_type
                .data
                .set(
                    *memo_type.data.get().expect(
                        "cannot provide an empty `MemoEntryType` for `MemoEntryType::set()`",
                    ),
                )
                .expect("memo type should only be set once");
            break;
        }
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

        let old_entry = mem::replace(
            memos[memo_ingredient_index].atomic_memo.get_mut(),
            MemoEntryType::to_dummy(memo).as_ptr(),
        );

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

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn memory_usage(&self) -> Vec<crate::database::MemoInfo> {
        let mut memory_usage = Vec::new();
        let memos = self.memos.memos.read();
        for (index, memo) in memos.iter().enumerate() {
            let Some(memo) = NonNull::new(memo.atomic_memo.load(Ordering::Acquire)) else {
                continue;
            };

            let Some(type_) = self.types.types.get(index).and_then(MemoEntryType::load) else {
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
        let Some(memo) = NonNull::new(*atomic_memo.get_mut()) else {
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
        let memo = mem::replace(self.atomic_memo.get_mut(), ptr::null_mut());
        let memo = NonNull::new(memo)?;
        let type_ = type_.load()?;
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
