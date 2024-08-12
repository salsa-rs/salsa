use std::{
    any::{Any, TypeId},
    cell::UnsafeCell,
    panic::RefUnwindSafe,
};

use append_only_vec::AppendOnlyVec;
use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

use crate::{zalsa::transmute_data_ptr, Id, IngredientIndex};

pub(crate) mod memo;

const PAGE_LEN_BITS: usize = 10;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;

pub(crate) struct Table {
    pages: AppendOnlyVec<Box<dyn TablePage>>,
}

pub(crate) trait TablePage: Any + Send + Sync {
    fn hidden_type_name(&self) -> &'static str;
}

pub(crate) struct Page<T: Any + Send + Sync> {
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

unsafe impl<T: Any + Send + Sync> Send for Page<T> {}

unsafe impl<T: Any + Send + Sync> Sync for Page<T> {}

impl<T: Any + Send + Sync> RefUnwindSafe for Page<T> {}

#[derive(Copy, Clone)]
pub struct PageIndex(usize);

#[derive(Copy, Clone)]
pub struct SlotIndex(usize);

impl Default for Table {
    fn default() -> Self {
        Self {
            pages: AppendOnlyVec::new(),
        }
    }
}

impl Table {
    pub fn get<T: Any + Send + Sync>(&self, id: Id) -> &T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        page_ref.get(slot)
    }

    pub fn get_raw<T: Any + Send + Sync>(&self, id: Id) -> *mut T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        page_ref.get_raw(slot)
    }

    pub fn page<T: Any + Send + Sync>(&self, page: PageIndex) -> &Page<T> {
        self.pages[page.0].assert_type::<Page<T>>()
    }

    pub fn push_page<T: Any + Send + Sync>(&self, ingredient: IngredientIndex) -> PageIndex {
        let page = Box::new(<Page<T>>::new(ingredient));
        PageIndex(self.pages.push(page))
    }
}

impl<T: Any + Send + Sync> Page<T> {
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

    pub(crate) fn get(&self, slot: SlotIndex) -> &T {
        let len = self.allocated.load();
        assert!(slot.0 < len);
        unsafe { &*self.data[slot.0].get() }
    }

    /// Returns a raw pointer to the given slot.
    /// Reads/writes must be coordinated properly with calls to `get`.
    pub(crate) fn get_raw(&self, slot: SlotIndex) -> *mut T {
        let len = self.allocated.load();
        assert!(slot.0 < len);
        self.data[slot.0].get()
    }

    pub(crate) fn allocate(&self, page: PageIndex, value: T) -> Result<Id, T> {
        let guard = self.allocation_lock.lock();
        let index = self.allocated.load();
        if index == PAGE_LEN {
            return Err(value);
        }

        // Initialize entry `index`
        let data = &self.data[index];
        unsafe { std::ptr::write(data.get(), value) };

        // Update the length (this must be done after initialization!)
        self.allocated.store(index + 1);
        drop(guard);

        Ok(make_id(page, SlotIndex(index)))
    }
}

impl<T: Any + Send + Sync> TablePage for Page<T> {
    fn hidden_type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl<T: Any + Send + Sync> Drop for Page<T> {
    fn drop(&mut self) {
        // Free `self.data` and the data within: to do this, we swap it out with an empty vector
        // and then convert it from a `Vec<UnsafeCell<T>>` with partially uninitialized values
        // to a `Vec<T>` with the correct length. This way the `Vec` drop impl can do its job.
        let mut data = std::mem::replace(&mut self.data, vec![]);
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
            self.type_id(),
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
