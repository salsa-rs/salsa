use std::{
    any::{Any, TypeId},
    fmt::Debug,
    mem::{self, ManuallyDrop},
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicPtr, Ordering},
        OnceLock,
    },
};

use parking_lot::RwLock;
use thin_vec::ThinVec;

use crate::{
    zalsa::{MemoIngredientIndex, Zalsa},
    zalsa_local::QueryOrigin,
};

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

#[derive(Default, Clone)]
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
    #[inline]
    fn to_dummy<M: Memo>(memo: NonNull<M>) -> NonNull<DummyMemo> {
        memo.cast()
    }

    #[inline]
    unsafe fn from_dummy<M: Memo>(memo: NonNull<DummyMemo>) -> NonNull<M> {
        memo.cast()
    }

    #[inline]
    const fn to_dyn_fn<M: Memo>() -> fn(NonNull<DummyMemo>) -> NonNull<dyn Memo> {
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

    #[inline]
    fn set(&self, new: &MemoEntryType) {
        self.data
            .set(
                *new.data
                    .get()
                    .expect("cannot provide an empty `MemoEntryType` for `MemoEntryType::set()`"),
            )
            .unwrap_or_else(|_| panic!("`MemoEntryType` was already set"));
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

/// # Safety
///
/// Only call this with `MemoTableTypes::types`.
#[inline]
unsafe fn to_memo_table_types_vec(ptr: *mut ()) -> ManuallyDrop<ThinVec<MemoEntryType>> {
    // SAFETY: The allowed types contain a valid `ThinVec`.
    unsafe { mem::transmute::<*mut (), ManuallyDrop<ThinVec<MemoEntryType>>>(ptr) }
}

#[inline]
fn from_memo_table_types_vec(vec: ThinVec<MemoEntryType>) -> *mut () {
    // SAFETY: They have the same layout.
    unsafe { mem::transmute::<ThinVec<MemoEntryType>, *mut ()>(vec) }
}

pub struct MemoTableTypes {
    /// This holds a `ThinVec`, replaced when we want to grow it, and old values are only garbage-collected
    /// during a revision bump. This way, we can do a simple atomic load to load the current types.
    types: AtomicPtr<()>,
}

impl Default for MemoTableTypes {
    #[inline]
    fn default() -> Self {
        let types = ThinVec::from_iter(
            [const {
                MemoEntryType {
                    data: OnceLock::new(),
                }
            }; 4],
        );
        MemoTableTypes {
            types: AtomicPtr::new(from_memo_table_types_vec(types)),
        }
    }
}

impl MemoTableTypes {
    #[inline]
    pub(crate) fn load(&self) -> &[MemoEntryType] {
        let types = self.types.load(Ordering::Acquire);
        // SAFETY: We are `MemoTableTypes`.
        let types = unsafe { to_memo_table_types_vec(types) };
        // SAFETY: The real data is stored in the `ThinVec` behind an indirection.
        unsafe { &*ptr::from_ref(&*types) }
    }

    #[inline]
    pub(crate) fn set(&self, index: MemoIngredientIndex, new: &MemoEntryType, zalsa: &Zalsa) {
        let index = index.as_usize();
        // Guard everything behind the lock, to make sure we're consistent.
        let mut garbage_types_guard = zalsa.garbage_memo_types.lock();

        let types = self.types.load(Ordering::Acquire);
        // SAFETY: We are `MemoTableTypes`.
        let types = unsafe { to_memo_table_types_vec(types) };
        if types.len() < index + 1 {
            let new_len = std::cmp::max(types.len() * 2, index + 1);
            let additional_len = new_len - types.len();
            let new_vec = types
                .iter()
                .cloned()
                .chain((0..additional_len).map(|_| MemoEntryType::default()))
                .collect();
            let new_types = from_memo_table_types_vec(new_vec);
            let old_types = self.types.swap(new_types, Ordering::Release);
            garbage_types_guard.push(MemoTableTypes {
                types: AtomicPtr::new(old_types),
            });
        }

        // This must still be done behind the lock, otherwise someone could replace the new types,
        // and we will set in the old types.
        self.load()[index].set(new);
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

impl Drop for MemoTableTypes {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: We are `MemoTableTypes`, and we are dropping so nobody will use us anymore.
        unsafe {
            drop(ManuallyDrop::into_inner(to_memo_table_types_vec(
                *self.types.get_mut(),
            )))
        };
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
        assert_eq!(
            self.types
                .load()
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
            assert_eq!(
                self.types
                    .load()
                    .get(memo_ingredient_index.as_usize())
                    .and_then(MemoEntryType::load)?
                    .type_id,
                TypeId::of::<M>(),
                "inconsistent type-id for `{memo_ingredient_index:?}`"
            );
            let memo = NonNull::new(atomic_memo.load(Ordering::Acquire));
            // SAFETY: `type_id` check asserted above
            return memo.map(|memo| unsafe { MemoEntryType::from_dummy(memo).as_ref() });
        }

        None
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
            .load()
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
    #[inline]
    pub fn drop(self) {
        let types = self.types.load().iter();
        for (type_, memo) in std::iter::zip(types, self.memos.memos.get_mut()) {
            // SAFETY: The types match because this is an invariant of `MemoTableWithTypesMut`.
            unsafe { memo.drop(type_) };
        }
    }

    /// # Safety
    ///
    /// The caller needs to make sure to not call this function until no more references into
    /// the database exist as there may be outstanding borrows into the pointer contents.
    pub(crate) unsafe fn with_memos(self, mut f: impl FnMut(MemoIngredientIndex, Box<dyn Memo>)) {
        let memos = self.memos.memos.get_mut();
        memos
            .iter_mut()
            .zip(self.types.load().iter())
            .zip(0..)
            .filter_map(|((memo, type_), index)| {
                let memo = mem::replace(memo.atomic_memo.get_mut(), ptr::null_mut());
                let memo = NonNull::new(memo)?;
                Some((memo, type_.load()?, index))
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
            if let Some(type_) = type_.load() {
                // SAFETY: Our preconditions.
                mem::drop(unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) });
            }
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
