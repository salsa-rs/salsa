use std::{
    any::{Any, TypeId},
    fmt::Debug,
    mem,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, Ordering},
};

use parking_lot::RwLock;
use thin_vec::ThinVec;

use crate::{zalsa::MemoIngredientIndex, zalsa_local::QueryOrigin};

/// The "memo table" stores the memoized results of tracked function calls.
/// Every tracked function must take a salsa struct as its first argument
/// and memo tables are attached to those salsa structs as auxiliary data.
#[derive(Default)]
pub(crate) struct MemoTable {
    memos: RwLock<ThinVec<MemoEntry>>,
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

#[derive(Clone, Copy)]
pub struct MemoEntryType {
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

    fn to_dyn_fn<M: Memo>() -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
        let f: fn(NonNull<M>) -> NonNull<dyn Memo> = |x| x;

        #[allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety
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
            type_id: TypeId::of::<M>(),
            to_dyn_fn: Self::to_dyn_fn::<M>(),
        }
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
    types: RwLock<Vec<Option<MemoEntryType>>>,
}

impl MemoTableTypes {
    pub(crate) fn set(&self, memo_ingredient_index: MemoIngredientIndex, memo_type: MemoEntryType) {
        let mut types = self.types.write();
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        if memo_ingredient_index >= types.len() {
            types.resize_with(memo_ingredient_index + 1, Default::default);
        }
        match &mut types[memo_ingredient_index] {
            Some(existing) => {
                assert_eq!(
                    existing.type_id, memo_type.type_id,
                    "inconsistent type-id for `{memo_ingredient_index:?}`"
                );
            }
            entry @ None => {
                *entry = Some(memo_type);
            }
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

impl<'a> MemoTableWithTypes<'a> {
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(crate) unsafe fn insert<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        // The type must already exist, we insert it when creating the memo ingredient.
        let types = self.types.types.read();
        assert_eq!(
            types[memo_ingredient_index.as_usize()]
                .as_ref()
                .expect("memo type should be available in insert")
                .type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );
        drop(types);

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
        // SAFETY: The caller is responsible for dropping
        unsafe { self.insert_cold(memo_ingredient_index, memo) }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    unsafe fn insert_cold<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        memo: NonNull<M>,
    ) -> Option<NonNull<M>> {
        let memo_ingredient_index = memo_ingredient_index.as_usize();
        let mut memos = self.memos.memos.write();
        let additional_len = memo_ingredient_index - memos.len() + 1;
        memos.reserve(additional_len);
        while memos.len() < memo_ingredient_index + 1 {
            memos.push(MemoEntry::default());
        }
        let old_entry = mem::replace(
            memos[memo_ingredient_index].atomic_memo.get_mut(),
            MemoEntryType::to_dummy(memo).as_ptr(),
        );
        let old_entry = NonNull::new(old_entry);
        // SAFETY: The `TypeId` is asserted in `insert()`.
        old_entry.map(|memo| unsafe { MemoEntryType::from_dummy(memo) })
    }

    pub(crate) fn get<M: Memo>(self, memo_ingredient_index: MemoIngredientIndex) -> Option<&'a M> {
        if let Some(MemoEntry { atomic_memo }) = self
            .memos
            .memos
            .read()
            .get(memo_ingredient_index.as_usize())
        {
            if let Some(Some(MemoEntryType {
                type_id,
                to_dyn_fn: _,
            })) = self
                .types
                .types
                .read()
                .get(memo_ingredient_index.as_usize())
            {
                assert_eq!(
                    *type_id,
                    TypeId::of::<M>(),
                    "inconsistent type-id for `{memo_ingredient_index:?}`"
                );
                let memo = NonNull::new(atomic_memo.load(Ordering::Acquire));
                // SAFETY: `type_id` check asserted above
                return memo.map(|memo| unsafe { MemoEntryType::from_dummy(memo).as_ref() });
            }
        }

        None
    }
}

pub(crate) struct MemoTableWithTypesMut<'a> {
    types: &'a MemoTableTypes,
    memos: &'a mut MemoTable,
}

impl<'a> MemoTableWithTypesMut<'a> {
    #[inline]
    pub(crate) fn reborrow<'b>(&'b mut self) -> MemoTableWithTypesMut<'b>
    where
        'a: 'b,
    {
        MemoTableWithTypesMut {
            types: self.types,
            memos: self.memos,
        }
    }

    /// Calls `f` on the memo at `memo_ingredient_index`.
    ///
    /// If the memo is not present, `f` is not called.
    pub(crate) fn map_memo<M: Memo>(
        self,
        memo_ingredient_index: MemoIngredientIndex,
        f: impl FnOnce(&mut M),
    ) {
        let types = self.types.types.read();
        let Some(Some(memo_type)) = types.get(memo_ingredient_index.as_usize()) else {
            return;
        };
        assert_eq!(
            memo_type.type_id,
            TypeId::of::<M>(),
            "inconsistent type-id for `{memo_ingredient_index:?}`"
        );
        drop(types);

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
    #[inline]
    pub fn drop(self) {
        let types = self.types.types.read();
        let types = types.iter();
        for (type_, memo) in std::iter::zip(types, self.memos.memos.get_mut()) {
            if let Some(type_) = type_ {
                // SAFETY: The types match because this is an invariant of `MemoTableWithTypesMut`.
                unsafe {
                    memo.drop(type_);
                }
            }
        }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not call this function until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    pub(crate) unsafe fn with_memos(self, mut f: impl FnMut(MemoIngredientIndex, Box<dyn Memo>)) {
        let memos = self.memos.memos.get_mut();
        let types = self.types.types.read();
        memos
            .iter_mut()
            .zip(types.iter())
            .zip(0..)
            .filter_map(|((memo, type_), index)| {
                let memo = mem::replace(memo.atomic_memo.get_mut(), ptr::null_mut());
                let memo = NonNull::new(memo)?;
                Some((memo, type_.as_ref()?, index))
            })
            .map(|(memo, type_, index)| {
                // SAFETY: We took ownership of the memo, and converted it to the correct type.
                // The caller guarantees that there are no outstanding borrows into the `Box` contents.
                let memo = unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) };
                (MemoIngredientIndex::from_usize(index), memo)
            })
            .for_each(|(index, memo)| f(index, memo));
    }
}

impl MemoEntry {
    /// # Safety
    ///
    /// The type must match.
    #[inline]
    unsafe fn drop(&mut self, type_: &MemoEntryType) {
        if let Some(memo) = NonNull::new(mem::replace(self.atomic_memo.get_mut(), ptr::null_mut()))
        {
            // SAFETY: Our preconditions.
            mem::drop(unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) });
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
        f.debug_struct("MemoTable").finish_non_exhaustive()
    }
}
