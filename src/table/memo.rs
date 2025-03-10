use std::{
    any::{Any, TypeId},
    fmt::Debug,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};

use parking_lot::RwLock;

use crate::{zalsa::MemoIngredientIndex, zalsa_local::QueryOrigin};

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<Vec<MemoEntry>>,
}

pub(crate) trait Memo: Any + Send + Sync + Debug {
    /// Returns the `origin` of this memo
    fn origin(&self) -> &QueryOrigin;
}

/// Wraps the data stored for a memoized entry.
/// This struct has a customized Drop that will
/// ensure that its `data` field is properly freed.
#[derive(Default)]
struct MemoEntry {
    data: Option<MemoEntryData>,
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
struct MemoEntryData {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A type-coercion function for the erased memo type `M`
    to_dyn_fn: fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>,

    /// An [`AtomicPtr`][] to a `Box<M>` for the erased memo type `M`
    atomic_memo: AtomicPtr<DummyMemo>,
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
struct DummyMemo {}

impl MemoTable {
    fn to_dummy<M: Memo>(memo: NonNull<M>) -> NonNull<DummyMemo> {
        memo.cast()
    }

    unsafe fn from_dummy<M: Memo>(memo: NonNull<DummyMemo>) -> NonNull<M> {
        memo.cast()
    }

    fn to_dyn_fn<M: Memo>() -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
        let f: fn(NonNull<M>) -> NonNull<dyn Memo> = |x| x;

        unsafe {
            std::mem::transmute::<
                fn(NonNull<M>) -> NonNull<dyn Memo>,
                fn(NonNull<DummyMemo>) -> NonNull<dyn Memo>,
            >(f)
        }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not free the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    pub(crate) unsafe fn insert<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        if let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
                    atomic_memo,
                }),
        }) = self.memos.read().get(memo_ingredient_index.as_usize())
        {
            assert_eq!(
                *type_id,
                TypeId::of::<M>(),
                "inconsistent type-id for `{memo_ingredient_index:?}`"
            );

            let old_memo = atomic_memo.swap(Self::to_dummy(memo).as_ptr(), Ordering::AcqRel);

            // SAFETY: The `atomic_memo` field is never null.
            let old_memo = unsafe { NonNull::new_unchecked(old_memo) };

            // SAFETY: `type_id` check asserted above
            return Some(unsafe { Self::from_dummy(old_memo) });
        }

        // Otherwise we need the write lock.
        // SAFETY: The caller is responsible for dropping
        unsafe { self.insert_cold(memo_ingredient_index, memo) }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not free the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    unsafe fn insert_cold<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        let mut memos = self.memos.write();
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        if memos.len() < memo_ingredient_index + 1 {
            memos.resize_with(memo_ingredient_index + 1, MemoEntry::default);
        }
        let old_entry = memos[memo_ingredient_index].data.replace(MemoEntryData {
            type_id: TypeId::of::<M>(),
            to_dyn_fn: Self::to_dyn_fn::<M>(),
            atomic_memo: AtomicPtr::new(Self::to_dummy(memo).as_ptr()),
        });
        old_entry.map(
            |MemoEntryData {
                 type_id: _,
                 to_dyn_fn: _,
                 atomic_memo,
             }| unsafe {
                // SAFETY: The `atomic_memo` field is never null.
                Self::from_dummy(NonNull::new_unchecked(atomic_memo.into_inner()))
            },
        )
    }

    pub(crate) fn get<M: Memo>(&self, memo_ingredient_index: MemoIngredientIndex) -> Option<&M> {
        let memos = self.memos.read();

        let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
                    atomic_memo,
                }),
        }) = memos.get(memo_ingredient_index.as_usize())
        else {
            return None;
        };

        assert_eq!(
            *type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );

        // SAFETY: The `atomic_memo` field is never null.
        let memo = unsafe { NonNull::new_unchecked(atomic_memo.load(Ordering::Acquire)) };

        // SAFETY: `type_id` check asserted above
        unsafe { Some(Self::from_dummy(memo).as_ref()) }
    }

    /// Calls `f` on the memo at `memo_ingredient_index`.
    ///
    /// If the memo is not present, `f` is not called.
    pub(crate) fn map_memo<M: Memo>(
        &mut self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(&mut M),
    ) {
        let memos = self.memos.get_mut();
        let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
                    atomic_memo,
                }),
        }) = memos.get_mut(memo_ingredient_index.as_usize())
        else {
            return;
        };

        assert_eq!(
            *type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );

        // SAFETY: The `atomic_memo` field is never null.
        let memo = unsafe { NonNull::new_unchecked(*atomic_memo.get_mut()) };

        // SAFETY: `type_id` check asserted above
        f(unsafe { Self::from_dummy(memo).as_mut() });
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not call this function until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    pub(crate) unsafe fn into_memos(self) -> impl Iterator<Item = Box<dyn Memo>> {
        self.memos
            .into_inner()
            .into_iter()
            .filter_map(|mut memo| memo.data.take())
            .map(
                |MemoEntryData {
                     type_id: _,
                     to_dyn_fn,
                     atomic_memo,
                 }| {
                    // SAFETY: The `atomic_memo` field is never null.
                    let memo =
                        unsafe { to_dyn_fn(NonNull::new_unchecked(atomic_memo.into_inner())) };
                    // SAFETY: The caller guarantees that there are no outstanding borrows into the `Box` contents.
                    unsafe { Box::from_raw(memo.as_ptr()) }
                },
            )
    }
}

impl Drop for MemoEntry {
    fn drop(&mut self) {
        if let Some(MemoEntryData {
            type_id: _,
            to_dyn_fn,
            atomic_memo,
        }) = self.data.take()
        {
            // SAFETY: The `atomic_memo` field is never null.
            let memo = unsafe { to_dyn_fn(NonNull::new_unchecked(atomic_memo.into_inner())) };
            // SAFETY: We have `&mut self`, so there are no outstanding borrows into the `Box` contents.
            let memo = unsafe { Box::from_raw(memo.as_ptr()) };
            std::mem::drop(memo);
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
