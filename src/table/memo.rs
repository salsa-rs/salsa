use std::{
    any::{Any, TypeId},
    cell::UnsafeCell,
    fmt::Debug,
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use append_only_vec::AppendOnlyVec;
use arc_swap::ArcSwapOption;
use parking_lot::{Mutex, RwLock};

use crate::{zalsa::MemoIngredientIndex, zalsa_local::QueryOrigin};

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<Vec<MemoEntry>>,
}

impl MemoTable {
    #[inline]
    pub unsafe fn drop(&mut self, types: &MemoTableTypes) {
        let types = types.iter();
        for (type_, memo) in std::iter::zip(types, self.memos.get_mut()) {
            if let Some(type_) = type_ {
                unsafe {
                    memo.drop(type_);
                }
            }
        }
    }
}

pub(crate) trait Memo: Any + Send + Sync + Debug {
    /// Returns the `origin` of this memo
    fn origin(&self) -> &QueryOrigin;
}

/// Data for a memoized entry.
/// This is a type-erased `Arc<M>`, where `M` is the type of memo associated
/// with that particular ingredient index.
///
/// # Implementation note
///
/// Every entry is associated with some ingredient that has been added to the database.
/// That ingredient has a fixed type of values that it produces etc.
/// Therefore, once a given entry goes from `Empty` to `Full`,
/// the type-id associated with that entry should never change.
///
/// We take advantage of this and use an `ArcSwap` to store the actual memo.
/// This allows us to store into the memo-entry without acquiring a write-lock.
/// However, using `ArcSwap` means we cannot use a `Arc<dyn Any>` or any other wide pointer.
/// Therefore, we hide the type by transmuting to `DummyMemo`; but we must then be very careful
/// when freeing `MemoEntryData` values to transmute things back.
#[derive(Default)]
struct MemoEntry {
    arc_swap: ManuallyDrop<ArcSwapOption<DummyMemo>>,
}

struct MemoEntryType {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A pointer to `std::mem::drop::<Arc<M>>` for the erased memo type `M`
    to_dyn_fn: fn(Arc<DummyMemo>) -> Arc<dyn Memo>,
}

/// Using `RwLock<Vec>` costs too much, unfortunately, so we're using a data structure
/// that is free for reads.
struct MemoEntryTypeLock {
    data: UnsafeCell<MemoEntryType>,
    /// The key is: as long as `initialized` is `false`, you cannot make any shared reference
    /// to `data` (because there might be writes), and every write must be guarded by `init_lock`.
    /// Once someone writes into `data`, they change `initialized` to true, and from this point
    /// only shared references to `data` are allowed, no mutation.
    initialized: AtomicBool,
    /// If `initialized` is `false`, we take this lock or block on it.
    init_lock: Mutex<()>,
}

// SAFETY: We handle synchronization.
unsafe impl Sync for MemoEntryTypeLock {}

#[derive(Default)]
pub struct MemoTableTypes {
    types: AppendOnlyVec<MemoEntryTypeLock>,
}

impl MemoTableTypes {
    fn get(&self, idx: usize) -> Option<&MemoEntryType> {
        if idx < self.types.len() {
            let value = &self.types[idx];
            let initialized = value.initialized.load(Ordering::Acquire);
            if initialized {
                // SAFETY: We checked this is initialized and we have proper synchronization.
                Some(unsafe { &*value.data.get() })
            } else {
                None
            }
        } else {
            None
        }
    }

    fn to_dyn_fn<M: Memo>() -> fn(Arc<DummyMemo>) -> Arc<dyn Memo> {
        let f: fn(Arc<M>) -> Arc<dyn Memo> = |x| x;
        unsafe {
            std::mem::transmute::<fn(Arc<M>) -> Arc<dyn Memo>, fn(Arc<DummyMemo>) -> Arc<dyn Memo>>(
                f,
            )
        }
    }

    fn push_empty(&self, len: usize) {
        self.types.extend((0..len).map(|_| MemoEntryTypeLock {
            data: UnsafeCell::new(MemoEntryType {
                type_id: TypeId::of::<DummyMemo>(),
                to_dyn_fn: Self::to_dyn_fn::<DummyMemo>(),
            }),
            initialized: AtomicBool::new(false),
            init_lock: Mutex::new(()),
        }));
    }

    fn set<M: Memo>(&self, memo_ingredient_index: MemoIngredientIndex) {
        let entry = &self.types[memo_ingredient_index.as_usize()];
        let _lock = entry.init_lock.lock();
        if entry.initialized.load(Ordering::Acquire) {
            // Someone raced with us, assert the type is correct.
            // SAFETY: We checked this is initialized and we have proper synchronization.
            let value = unsafe { &*entry.data.get() };
            assert_eq!(
                value.type_id,
                TypeId::of::<M>(),
                "inconsistent type-id for `{memo_ingredient_index:?}`"
            );
            return;
        }

        // SAFETY: We have proper synchronization.
        // No-one writes until they have the lock, and we hold it.
        unsafe {
            *entry.data.get() = MemoEntryType {
                type_id: TypeId::of::<M>(),
                to_dyn_fn: Self::to_dyn_fn::<M>(),
            };
        }
        entry.initialized.store(true, Ordering::Release);
    }

    fn iter(&self) -> impl Iterator<Item = Option<&MemoEntryType>> {
        self.types.iter().map(|ty| {
            if ty.initialized.load(Ordering::Acquire) {
                // SAFETY: We checked this is initialized and we have proper synchronization.
                unsafe { Some(&*ty.data.get()) }
            } else {
                None
            }
        })
    }
}

impl MemoTableTypes {
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
}

pub(crate) struct MemoTableWithTypes<'a> {
    types: &'a MemoTableTypes,
    memos: &'a MemoTable,
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
#[derive(Debug)]
struct DummyMemo {}

impl Memo for DummyMemo {
    fn origin(&self) -> &QueryOrigin {
        unreachable!("should not get here")
    }
}

impl MemoTableWithTypes<'_> {
    fn to_dummy<M: Memo>(memo: Arc<M>) -> Arc<DummyMemo> {
        unsafe { std::mem::transmute::<Arc<M>, Arc<DummyMemo>>(memo) }
    }

    unsafe fn from_dummy<M: Memo>(memo: Arc<DummyMemo>) -> Arc<M> {
        unsafe { std::mem::transmute::<Arc<DummyMemo>, Arc<M>>(memo) }
    }

    pub(crate) fn insert<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) -> Option<Arc<M>> {
        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        // Leave the ifs in this order, because the types entry has more chance to be occupied
        // than the memo entry, so put it last to save work.
        if let Some(MemoEntry { arc_swap }) = self
            .memos
            .memos
            .read()
            .get(memo_ingredient_index.as_usize())
        {
            if let Some(MemoEntryType {
                type_id,
                to_dyn_fn: _,
            }) = self.types.get(memo_ingredient_index.as_usize())
            {
                assert_eq!(
                    *type_id,
                    TypeId::of::<M>(),
                    "inconsistent type-id for `{memo_ingredient_index:?}`"
                );
                let old_memo = arc_swap.swap(Some(Self::to_dummy(memo)));
                return unsafe { old_memo.map(|memo| Self::from_dummy(memo)) };
            }
        }

        // Otherwise we need the write lock.
        self.insert_cold(memo_ingredient_index, memo)
    }

    fn insert_cold<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) -> Option<Arc<M>> {
        let memo_ingredient_index_usize = memo_ingredient_index.as_usize();
        let mut memos = self.memos.memos.write();
        if memos.len() < memo_ingredient_index_usize + 1 {
            memos.resize_with(memo_ingredient_index_usize + 1, MemoEntry::default);
        }
        let types_len = self.types.types.len();
        if types_len < memo_ingredient_index_usize + 1 {
            self.types
                .push_empty(memo_ingredient_index_usize + 1 - types_len);
        }
        self.types.set::<M>(memo_ingredient_index);
        let old_entry = memos[memo_ingredient_index_usize]
            .arc_swap
            .swap(Some(Self::to_dummy(memo)));
        unsafe { old_entry.map(|memo| Self::from_dummy(memo)) }
    }

    pub(crate) fn get<M: Memo>(self, memo_ingredient_index: MemoIngredientIndex) -> Option<Arc<M>> {
        if let Some(MemoEntry { arc_swap }) = self
            .memos
            .memos
            .read()
            .get(memo_ingredient_index.as_usize())
        {
            if let Some(MemoEntryType {
                type_id,
                to_dyn_fn: _,
            }) = self.types.get(memo_ingredient_index.as_usize())
            {
                assert_eq!(
                    *type_id,
                    TypeId::of::<M>(),
                    "inconsistent type-id for `{memo_ingredient_index:?}`"
                );
                return unsafe { arc_swap.load_full().map(|memo| Self::from_dummy(memo)) };
            }
        }

        None
    }

    /// Calls `f` on the memo at `memo_ingredient_index` and replaces the memo with the result of `f`.
    /// If the memo is not present, `f` is not called.
    pub(crate) fn map_memo<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(Arc<M>) -> Arc<M>,
    ) {
        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        let memos = self.memos.memos.read();
        let Some(MemoEntry { arc_swap }) = memos.get(memo_ingredient_index.as_usize()) else {
            return;
        };
        let Some(MemoEntryType {
            type_id,
            to_dyn_fn: _,
        }) = self.types.get(memo_ingredient_index.as_usize())
        else {
            return;
        };
        assert_eq!(
            *type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );
        let Some(arc) = arc_swap.load_full() else {
            return;
        };
        // SAFETY: type_id check asserted above
        let memo = f(unsafe { Self::from_dummy(arc) });
        unsafe {
            arc_swap
                .swap(Some(Self::to_dummy(memo)))
                .map(|memo| drop(Self::from_dummy::<M>(memo)))
        };
    }

    pub(crate) fn with_memos(self, mut f: impl FnMut(MemoIngredientIndex, Arc<dyn Memo>)) {
        let memos = self.memos.memos.read();
        memos
            .iter()
            .zip(self.types.iter())
            .zip(0..)
            .filter_map(|((memo, type_), index)| Some((memo.arc_swap.swap(None)?, type_?, index)))
            .map(|(arc_swap, type_, index)| {
                (
                    MemoIngredientIndex::from_usize(index),
                    (type_.to_dyn_fn)(arc_swap),
                )
            })
            .for_each(|(index, memo)| f(index, memo));
    }
}

impl MemoEntry {
    /// # Safety
    ///
    /// The type must match.
    unsafe fn drop(&mut self, type_: &MemoEntryType) {
        if let Some(memo) =
            std::mem::replace(&mut *self.arc_swap, ArcSwapOption::empty()).into_inner()
        {
            std::mem::drop((type_.to_dyn_fn)(memo));
        }
    }
}

impl Drop for DummyMemo {
    fn drop(&mut self) {
        unreachable!("should never get here")
    }
}

impl std::fmt::Debug for MemoTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoTable").finish()
    }
}
