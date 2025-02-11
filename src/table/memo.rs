use std::{
    any::{Any, TypeId},
    fmt::Debug,
    mem::ManuallyDrop,
    sync::Arc,
};

use arc_swap::ArcSwap;
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
    to_dyn_fn: fn(Arc<DummyMemo>) -> Arc<dyn Memo>,

    /// An [`ArcSwap`][] to a `Arc<M>` for the erased memo type `M`
    arc_swap: ArcSwap<DummyMemo>,
}

/// Dummy placeholder type that we use when erasing the memo type `M` in [`MemoEntryData`][].
struct DummyMemo {}

impl MemoTable {
    fn to_dummy<M: Memo>(memo: Arc<M>) -> Arc<DummyMemo> {
        unsafe { std::mem::transmute::<Arc<M>, Arc<DummyMemo>>(memo) }
    }

    unsafe fn from_dummy<M: Memo>(memo: Arc<DummyMemo>) -> Arc<M> {
        unsafe { std::mem::transmute::<Arc<DummyMemo>, Arc<M>>(memo) }
    }

    fn to_dyn_fn<M: Memo>() -> fn(Arc<DummyMemo>) -> Arc<dyn Memo> {
        let f: fn(Arc<M>) -> Arc<dyn Memo> = |x| x;
        unsafe {
            std::mem::transmute::<fn(Arc<M>) -> Arc<dyn Memo>, fn(Arc<DummyMemo>) -> Arc<dyn Memo>>(
                f,
            )
        }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(crate) unsafe fn insert<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) -> Option<ManuallyDrop<Arc<M>>> {
        // If the memo slot is already occupied, it must already have the
        // right type info etc, and we only need the read-lock.
        if let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
                    arc_swap,
                }),
        }) = self.memos.read().get(memo_ingredient_index.as_usize())
        {
            assert_eq!(
                *type_id,
                TypeId::of::<M>(),
                "inconsistent type-id for `{memo_ingredient_index:?}`"
            );
            let old_memo = arc_swap.swap(Self::to_dummy(memo));
            return Some(ManuallyDrop::new(unsafe { Self::from_dummy(old_memo) }));
        }

        // Otherwise we need the write lock.
        // SAFETY: The caller is responsible for dropping
        unsafe { self.insert_cold(memo_ingredient_index, memo) }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    unsafe fn insert_cold<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: Arc<M>,
    ) -> Option<ManuallyDrop<Arc<M>>> {
        let mut memos = self.memos.write();
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        if memos.len() < memo_ingredient_index + 1 {
            memos.resize_with(memo_ingredient_index + 1, MemoEntry::default);
        }
        let old_entry = std::mem::replace(
            &mut memos[memo_ingredient_index].data,
            Some(MemoEntryData {
                type_id: TypeId::of::<M>(),
                to_dyn_fn: Self::to_dyn_fn::<M>(),
                arc_swap: ArcSwap::new(Self::to_dummy(memo)),
            }),
        );
        old_entry
            .map(
                |MemoEntryData {
                     type_id: _,
                     to_dyn_fn: _,
                     arc_swap,
                 }| unsafe { Self::from_dummy(arc_swap.into_inner()) },
            )
            .map(ManuallyDrop::new)
    }

    pub(crate) fn get<M: Memo>(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<Arc<M>> {
        let memos = self.memos.read();

        let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
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

    /// Calls `f` on the memo at `memo_ingredient_index` and replaces the memo with the result of `f`.
    /// If the memo is not present, `f` is not called.
    ///
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(crate) unsafe fn map_memo<M: Memo>(
        &mut self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(Arc<M>) -> Arc<M>,
    ) -> Option<ManuallyDrop<Arc<M>>> {
        let memos = self.memos.get_mut();
        let Some(MemoEntry {
            data:
                Some(MemoEntryData {
                    type_id,
                    to_dyn_fn: _,
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
        // arc-swap does not expose accessing the interior mutably at all unfortunately
        // https://github.com/vorner/arc-swap/issues/131
        // so we are required to allocate a nwe arc within `f` instead of being able
        // to swap out the interior
        // SAFETY: type_id check asserted above
        let memo = f(unsafe { Self::from_dummy(arc_swap.load_full()) });
        Some(ManuallyDrop::new(unsafe {
            Self::from_dummy::<M>(arc_swap.swap(Self::to_dummy(memo)))
        }))
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(crate) unsafe fn into_memos(
        self,
    ) -> impl Iterator<Item = (MemoIngredientIndex, ManuallyDrop<Arc<dyn Memo>>)> {
        self.memos
            .into_inner()
            .into_iter()
            .zip(0..)
            .filter_map(|(mut memo, index)| memo.data.take().map(|d| (d, index)))
            .map(
                |(
                    MemoEntryData {
                        type_id: _,
                        to_dyn_fn,
                        arc_swap,
                    },
                    index,
                )| {
                    (
                        MemoIngredientIndex::from_usize(index),
                        ManuallyDrop::new(to_dyn_fn(arc_swap.into_inner())),
                    )
                },
            )
    }
}

impl Drop for MemoEntry {
    fn drop(&mut self) {
        if let Some(MemoEntryData {
            type_id: _,
            to_dyn_fn,
            arc_swap,
        }) = self.data.take()
        {
            let arc = arc_swap.into_inner();
            std::mem::drop(to_dyn_fn(arc));
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
