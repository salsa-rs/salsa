use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use arc_swap::ArcSwap;
use parking_lot::RwLock;

use crate::zalsa::MemoIngredientIndex;

#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<Vec<MemoEntry>>,
}

/// Wraps the data stored for a memoized entry.
/// This struct has a customized Drop that will
/// ensure that its `data` field is properly freed.
#[derive(Default)]
struct MemoEntry {
    data: Option<MemoEntryData>,
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
/// when freeing `MemoEntryData` values to transmute things back. See the `Drop` impl for
/// [`MemoEntry`][] for details.
struct MemoEntryData {
    /// The `type_id` of the erased memo type `M`
    type_id: TypeId,

    /// A pointer to `std::mem::drop::<Arc<M>>` for the erased memo type `M`
    to_dyn_any_fn: fn(Arc<DummyMemo>) -> Arc<dyn Any>,

    /// An [`ArcSwap`][] to a `Arc<M>` for the erased memo type `M`
    arc_swap: ArcSwap<DummyMemo>,
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
enum DummyMemo {}

impl MemoTable {
    fn to_dummy<M>(memo: Arc<M>) -> Arc<DummyMemo> {
        unsafe { std::mem::transmute::<Arc<M>, Arc<DummyMemo>>(memo) }
    }

    unsafe fn from_dummy<M>(memo: Arc<DummyMemo>) -> Arc<M> {
        unsafe { std::mem::transmute::<Arc<DummyMemo>, Arc<M>>(memo) }
    }

    fn to_dyn_any_fn<M: Any>() -> fn(Arc<DummyMemo>) -> Arc<dyn Any> {
        let f: fn(Arc<M>) -> Arc<dyn Any> = |x| x;
        unsafe {
            std::mem::transmute::<fn(Arc<M>) -> Arc<dyn Any>, fn(Arc<DummyMemo>) -> Arc<dyn Any>>(f)
        }
    }

    pub(crate) fn insert<M: Any + Send + Sync>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) {
        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        if let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_any_fn: _,
                    arc_swap,
                }),
        }) = self.memos.read().get(memo_ingredient_index.as_usize())
        {
            assert_eq!(
                *type_id,
                TypeId::of::<M>(),
                "inconsistent type-id for `{memo_ingredient_index:?}`"
            );
            arc_swap.store(Self::to_dummy(memo));
            return;
        }

        // Otherwise we need the write lock.
        self.insert_cold(memo_ingredient_index, memo)
    }

    fn insert_cold<M: Any + Send + Sync>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) {
        let mut memos = self.memos.write();
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        memos.resize_with(memo_ingredient_index + 1, || MemoEntry::default());
        memos[memo_ingredient_index] = MemoEntry {
            data: Some(MemoEntryData {
                type_id: TypeId::of::<M>(),
                to_dyn_any_fn: Self::to_dyn_any_fn::<M>(),
                arc_swap: ArcSwap::new(Self::to_dummy(memo)),
            }),
        };
    }

    pub(crate) fn get<M: Any + Send + Sync>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<Arc<M>> {
        let memos = self.memos.read();

        let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_any_fn: _,
                    arc_swap,
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

        // SAFETY: type_id check asserted above
        unsafe { Some(Self::from_dummy(arc_swap.load_full())) }
    }
}

impl Drop for MemoEntry {
    fn drop(&mut self) {
        if let Some(MemoEntryData {
            type_id: _,
            to_dyn_any_fn,
            arc_swap,
        }) = self.data.take()
        {
            let arc = arc_swap.into_inner();
            std::mem::drop(to_dyn_any_fn(arc));
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
