use std::{
    any::{Any, TypeId},
    cell::UnsafeCell,
    mem::MaybeUninit,
    panic::RefUnwindSafe,
    ptr, slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use append_only_vec::AppendOnlyVec;
use memo::MemoTable;
use parking_lot::Mutex;

use crate::{
    table::memo::MemoTableTypes,
    zalsa::{transmute_data_ptr, Zalsa},
    Id, IngredientIndex, Revision,
};

pub(crate) mod memo;

const PAGE_LEN_BITS: usize = 10;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;
const MAX_PAGES: usize = 1 << (32 - PAGE_LEN_BITS);

pub(crate) struct Table {
    pages: AppendOnlyVec<Box<dyn TablePage>>,
}

impl Table {
    pub(crate) fn drop(self, zalsa: &Zalsa) {
        for mut page in self.pages.into_vec() {
            // SAFETY: We own `self` and don't use it after.
            unsafe {
                TablePage::drop(&mut *page, zalsa);
            }
        }
    }
}

pub(crate) trait TablePage: Any + Send + Sync {
    fn hidden_type_name(&self) -> &'static str;

    fn ingredient_index(&self) -> IngredientIndex;

    /// Access the memos attached to `slot`.
    ///
    /// # Safety condition
    ///
    /// The `current_revision` MUST be the current revision of the database owning this table page.
    unsafe fn memos_and_ingredient(
        &self,
        slot: SlotIndex,
        current_revision: Revision,
    ) -> (&MemoTable, IngredientIndex);

    /// # Safety
    ///
    /// You cannot use `self` after calling this method.
    unsafe fn drop(&mut self, zalsa: &Zalsa);
}

pub(crate) struct Page<T: Slot> {
    /// The ingredient for elements on this page.
    ingredient: IngredientIndex,

    /// Number of elements of `data` that are initialized.
    allocated: AtomicUsize,

    /// The "allocation lock" is held when we allocate a new entry.
    ///
    /// It ensures that we can load the index, initialize it, and then update the length atomically
    /// with respect to other allocations.
    ///
    /// We could avoid it if we wanted, we'd just have to be a bit fancier in our reasoning
    /// (for example, the bounds check in `Page::get` no longer suffices to truly guarantee
    /// that the data is initialized).
    allocation_lock: Mutex<()>,

    /// The potentially uninitialized data of this page. As we initialize new entries, we increment `allocated`.
    data: Box<[UnsafeCell<MaybeUninit<T>>; PAGE_LEN]>,

    /// A helper to ensure we don't forget to call `drop()`. It will panic if we forget or if we call it twice.
    dropped: bool,
}

pub(crate) trait Slot: Any + Send + Sync {
    /// Access the [`MemoTable`][] for this slot.
    ///
    /// # Safety condition
    ///
    /// The current revision MUST be the current revision of the database containing this slot.
    unsafe fn memos(&self, current_revision: Revision) -> &MemoTable;

    /// # Safety
    ///
    /// `types` must be correct.
    unsafe fn drop_memos(&mut self, types: &MemoTableTypes);
}

unsafe impl<T: Slot> Send for Page<T> {}

unsafe impl<T: Slot> Sync for Page<T> {}

impl<T: Slot> RefUnwindSafe for Page<T> {}

#[derive(Copy, Clone, Debug)]
pub struct PageIndex(usize);

impl PageIndex {
    fn new(idx: usize) -> Self {
        assert!(idx < MAX_PAGES);
        Self(idx)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SlotIndex(usize);

impl SlotIndex {
    fn new(idx: usize) -> Self {
        assert!(idx < PAGE_LEN);
        Self(idx)
    }
}

impl Default for Table {
    fn default() -> Self {
        Self {
            pages: AppendOnlyVec::new(),
        }
    }
}

impl Table {
    /// Returns the [`IngredientIndex`] for an [`Id`].
    #[inline]
    pub fn ingredient_index(&self, id: Id) -> IngredientIndex {
        let (page_idx, _) = split_id(id);
        self.pages[page_idx.0].ingredient_index()
    }

    /// Get a reference to the data for `id`, which must have been allocated from this table with type `T`.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    pub fn get<T: Slot>(&self, id: Id) -> &T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        page_ref.get(slot)
    }

    /// Get a raw pointer to the data for `id`, which must have been allocated from this table.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    ///
    /// # Safety
    ///
    /// See [`Page::get_raw`][].
    pub fn get_raw<T: Slot>(&self, id: Id) -> *mut T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        page_ref.get_raw(slot)
    }

    /// Gets a reference to the page which has slots of type `T`
    ///
    /// # Panics
    ///
    /// If `page` is out of bounds or the type `T` is incorrect.
    pub fn page<T: Slot>(&self, page: PageIndex) -> &Page<T> {
        self.pages[page.0].assert_type::<Page<T>>()
    }

    /// Allocate a new page for the given ingredient and with slots of type `T`
    pub fn push_page<T: Slot>(&self, ingredient: IngredientIndex) -> PageIndex {
        let page = Box::new(<Page<T>>::new(ingredient));
        PageIndex::new(self.pages.push(page))
    }

    /// Get the memo table associated with `id`
    ///
    /// # Safety condition
    ///
    /// The parameter `current_revision` MUST be the current revision
    /// of the owner of database owning this table.
    pub unsafe fn memos_and_ingredient(
        &self,
        id: Id,
        current_revision: Revision,
    ) -> (&MemoTable, IngredientIndex) {
        let (page, slot) = split_id(id);
        let page = &self.pages[page.0];
        page.memos_and_ingredient(slot, current_revision)
    }
}

impl<T: Slot> Page<T> {
    #[allow(clippy::uninit_vec)]
    fn new(ingredient: IngredientIndex) -> Self {
        Self {
            ingredient,
            allocated: Default::default(),
            allocation_lock: Default::default(),
            data: Box::new([const { UnsafeCell::new(MaybeUninit::uninit()) }; PAGE_LEN]),
            dropped: false,
        }
    }

    fn check_bounds(&self, slot: SlotIndex) {
        let len = self.allocated.load(Ordering::Acquire);
        assert!(
            slot.0 < len,
            "out of bounds access `{slot:?}` (maximum slot `{len}`)"
        );
    }

    /// Returns a reference to the given slot.
    ///
    /// # Panics
    ///
    /// If slot is out of bounds
    pub(crate) fn get(&self, slot: SlotIndex) -> &T {
        self.check_bounds(slot);
        unsafe { (*self.data[slot.0].get()).assume_init_ref() }
    }

    /// Returns a raw pointer to the given slot.
    ///
    /// # Panics
    ///
    /// If slot is out of bounds
    ///
    /// # Safety
    ///
    /// Safe to call, but reads/writes through this pointer must be coordinated
    /// properly with calls to [`get`](`Self::get`) and [`get_mut`](`Self::get_mut`).
    pub(crate) fn get_raw(&self, slot: SlotIndex) -> *mut T {
        self.check_bounds(slot);
        self.data[slot.0].get().cast()
    }

    pub(crate) fn allocate<V>(&self, page: PageIndex, value: V) -> Result<Id, V>
    where
        V: FnOnce(Id) -> T,
    {
        let guard = self.allocation_lock.lock();
        let index = self.allocated.load(Ordering::Acquire);
        if index == PAGE_LEN {
            return Err(value);
        }

        // Initialize entry `index`
        let id = make_id(page, SlotIndex::new(index));
        let data = &self.data[index];
        unsafe { (*data.get()).write(value(id)) };

        // Update the length (this must be done after initialization!)
        self.allocated.store(index + 1, Ordering::Release);
        drop(guard);

        Ok(id)
    }
}

impl<T: Slot> TablePage for Page<T> {
    fn hidden_type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient
    }

    unsafe fn memos_and_ingredient(
        &self,
        slot: SlotIndex,
        current_revision: Revision,
    ) -> (&MemoTable, IngredientIndex) {
        (self.get(slot).memos(current_revision), self.ingredient)
    }

    unsafe fn drop(&mut self, zalsa: &Zalsa) {
        assert!(!self.dropped, "cannot drop a Page twice");
        self.dropped = true;
        let types = zalsa.lookup_ingredient(self.ingredient).memo_table_types();
        // Execute destructors for all initialized elements
        let len = *self.allocated.get_mut();
        // SAFETY: self.data is initialized for T's up to len
        unsafe {
            // FIXME: Should be ptr::from_raw_parts_mut but that is unstable
            let to_drop = slice::from_raw_parts_mut(self.data.as_mut_ptr().cast::<T>(), len);
            for v in &mut *to_drop {
                v.drop_memos(types);
            }
            ptr::drop_in_place(to_drop);
        }
    }
}

impl<T: Slot> Drop for Page<T> {
    fn drop(&mut self) {
        assert!(self.dropped, "Page not dropped");
    }
}

impl dyn TablePage {
    fn assert_type<T: Any>(&self) -> &T {
        assert_eq!(
            Any::type_id(self),
            TypeId::of::<T>(),
            "page has hidden type `{:?}` but `{:?}` was expected",
            self.hidden_type_name(),
            std::any::type_name::<T>(),
        );

        // SAFETY: Assertion above
        unsafe { transmute_data_ptr::<dyn TablePage, T>(self) }
    }
}

fn make_id(page: PageIndex, slot: SlotIndex) -> Id {
    let page = page.0 as u32;
    let slot = slot.0 as u32;
    Id::from_u32((page << PAGE_LEN_BITS) | slot)
}

fn split_id(id: Id) -> (PageIndex, SlotIndex) {
    let id = id.as_u32() as usize;
    let slot = id & PAGE_LEN_MASK;
    let page = id >> PAGE_LEN_BITS;
    (PageIndex::new(page), SlotIndex::new(slot))
}
