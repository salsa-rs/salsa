use std::{any::Any, mem::MaybeUninit};

use append_only_vec::AppendOnlyVec;
use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

use crate::{Id, IngredientIndex};

const PAGE_LEN_BITS: usize = 10;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;

pub struct Table {
    pages: AppendOnlyVec<Box<dyn Any + Send + Sync>>,
}

pub struct Page<T: Any + Send + Sync> {
    /// The ingredient for elements on this page.
    #[allow(dead_code)] // pretty sure we'll need this eventually
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
    data: Vec<MaybeUninit<T>>,
}

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

    pub fn page<T: Any + Send + Sync>(&self, page: PageIndex) -> &Page<T> {
        self.pages[page.0].downcast_ref::<Page<T>>().unwrap()
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
        unsafe { self.data[slot.0].assume_init_ref() }
    }

    pub(crate) fn allocate(&self, page: PageIndex, value: T) -> Result<Id, T> {
        let guard = self.allocation_lock.lock();
        let index = self.allocated.load();
        if index == PAGE_LEN {
            return Err(value);
        }

        let data = &self.data[index];
        let data = data.as_ptr() as *mut T;
        unsafe { std::ptr::write(data, value) };

        self.allocated.store(index + 1);
        drop(guard);

        Ok(make_id(page, SlotIndex(index)))
    }
}

impl<T: Any + Send + Sync> Drop for Page<T> {
    fn drop(&mut self) {
        let mut data = std::mem::replace(&mut self.data, vec![]);
        let len = self.allocated.load();
        unsafe {
            data.set_len(len);
            drop(std::mem::transmute::<Vec<MaybeUninit<T>>, Vec<T>>(data));
        }
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
