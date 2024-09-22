use std::{
    any::{Any, TypeId},
    cell::UnsafeCell,
    panic::RefUnwindSafe,
};

use append_only_vec::AppendOnlyVec;
use crossbeam::atomic::AtomicCell;
use memo::MemoTable;
use parking_lot::Mutex;
use sync::SyncTable;

use crate::{zalsa::transmute_data_ptr, Id, IngredientIndex, Revision};

pub(crate) mod memo;
pub(crate) mod sync;
mod util;

const PAGE_LEN_BITS: usize = 10;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;

pub(crate) struct Table {
    pages: AppendOnlyVec<Box<dyn TablePage>>,
}

pub(crate) trait TablePage: Any + Send + Sync {
    fn hidden_type_name(&self) -> &'static str;

    /// Access the memos attached to `slot`.
    ///
    /// # Safety condition
    ///
    /// The `current_revision` MUST be the current revision of the database owning this table page.
    unsafe fn memos(&self, slot: SlotIndex, current_revision: Revision) -> &MemoTable;

    /// Access the syncs attached to `slot`.
    ///
    /// # Safety condition
    ///
    /// The `current_revision` MUST be the current revision of the database owning this table page.
    unsafe fn syncs(&self, slot: SlotIndex, current_revision: Revision) -> &SyncTable;
}

pub(crate) struct Page<T: Slot> {
    /// The ingredient for elements on this page.
    #[allow(dead_code)] // pretty sure we'll need this
    ingredient: IngredientIndex,

    /// Number of elements of `data` that are initialized.
    allocated: AtomicCell<usize>,

    /// The "allocation lock" is held when we allocate a new entry.
    ///
    /// It ensures that we can load the index, initialize it, and then update the length atomically
    /// with respect to other allocations.
    ///
    /// We could avoid it if we wanted, we'd just have to be a bit fancier in our reasoning
    /// (for example, the bounds check in `Page::get` no longer suffices to truly guarantee
    /// that the data is initialized).
    allocation_lock: Mutex<()>,

    /// Vector with data. This is always created with the capacity/length of `PAGE_LEN`
    /// and uninitialized data. As we initialize new entries, we increment `allocated`.
    data: Vec<UnsafeCell<T>>,
}

pub(crate) trait Slot: Any + Send + Sync {
    /// Access the [`MemoTable`][] for this slot.
    ///
    /// # Safety condition
    ///
    /// The current revision MUST be the current revision of the database containing this slot.
    unsafe fn memos(&self, current_revision: Revision) -> &MemoTable;

    /// Access the [`SyncTable`][] for this slot.
    ///
    /// # Safety condition
    ///
    /// The current revision MUST be the current revision of the database containing this slot.
    unsafe fn syncs(&self, current_revision: Revision) -> &SyncTable;
}

unsafe impl<T: Slot> Send for Page<T> {}

unsafe impl<T: Slot> Sync for Page<T> {}

impl<T: Slot> RefUnwindSafe for Page<T> {}

#[derive(Copy, Clone, Debug)]
pub struct PageIndex(usize);

#[derive(Copy, Clone, Debug)]
pub struct SlotIndex(usize);

impl Default for Table {
    fn default() -> Self {
        Self {
            pages: AppendOnlyVec::new(),
        }
    }
}

impl Table {
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
        PageIndex(self.pages.push(page))
    }

    /// Get the memo table associated with `id`
    ///
    /// # Safety condition
    ///
    /// The parameter `current_revision` MUST be the current revision
    /// of the owner of database owning this table.
    pub unsafe fn memos(&self, id: Id, current_revision: Revision) -> &MemoTable {
        let (page, slot) = split_id(id);
        self.pages[page.0].memos(slot, current_revision)
    }

    /// Get the sync table associated with `id`
    ///
    /// # Safety condition
    ///
    /// The parameter `current_revision` MUST be the current revision
    /// of the owner of database owning this table.
    pub unsafe fn syncs(&self, id: Id, current_revision: Revision) -> &SyncTable {
        let (page, slot) = split_id(id);
        self.pages[page.0].syncs(slot, current_revision)
    }
}

impl<T: Slot> Page<T> {
    #[allow(clippy::uninit_vec)]
    fn new(ingredient: IngredientIndex) -> Self {
        let mut data = Vec::with_capacity(PAGE_LEN);
        unsafe {
            data.set_len(PAGE_LEN);
        }
        Self {
            ingredient,
            allocated: Default::default(),
            allocation_lock: Default::default(),
            data,
        }
    }

    fn check_bounds(&self, slot: SlotIndex) {
        let len = self.allocated.load();
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
        unsafe { &*self.data[slot.0].get() }
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
        self.data[slot.0].get()
    }

    pub(crate) fn allocate<V>(&self, page: PageIndex, value: V) -> Result<Id, V>
    where
        V: FnOnce() -> T,
    {
        let guard = self.allocation_lock.lock();
        let index = self.allocated.load();
        if index == PAGE_LEN {
            return Err(value);
        }

        // Initialize entry `index`
        let data = &self.data[index];
        unsafe { std::ptr::write(data.get(), value()) };

        // Update the length (this must be done after initialization!)
        self.allocated.store(index + 1);
        drop(guard);

        Ok(make_id(page, SlotIndex(index)))
    }
}

impl<T: Slot> TablePage for Page<T> {
    fn hidden_type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    unsafe fn memos(&self, slot: SlotIndex, current_revision: Revision) -> &MemoTable {
        self.get(slot).memos(current_revision)
    }

    unsafe fn syncs(&self, slot: SlotIndex, current_revision: Revision) -> &SyncTable {
        self.get(slot).syncs(current_revision)
    }
}

impl<T: Slot> Drop for Page<T> {
    fn drop(&mut self) {
        // Free `self.data` and the data within: to do this, we swap it out with an empty vector
        // and then convert it from a `Vec<UnsafeCell<T>>` with partially uninitialized values
        // to a `Vec<T>` with the correct length. This way the `Vec` drop impl can do its job.
        let mut data = std::mem::take(&mut self.data);
        let len = self.allocated.load();
        unsafe {
            data.set_len(len);
            drop(std::mem::transmute::<Vec<UnsafeCell<T>>, Vec<T>>(data));
        }
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
    assert!(slot.0 < PAGE_LEN);
    assert!(page.0 < (1 << (32 - PAGE_LEN_BITS)));
    let page = page.0 as u32;
    let slot = slot.0 as u32;
    Id::from_u32(page << PAGE_LEN_BITS | slot)
}

fn split_id(id: Id) -> (PageIndex, SlotIndex) {
    let id = id.as_u32() as usize;
    let slot = id & PAGE_LEN_MASK;
    let page = id >> PAGE_LEN_BITS;
    (PageIndex(page), SlotIndex(slot))
}
