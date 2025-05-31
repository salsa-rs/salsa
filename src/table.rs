use std::alloc::Layout;
use std::any::{Any, TypeId};
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr::{self, NonNull};
use std::slice;

use memo::MemoTable;
use rustc_hash::FxHashMap;

use crate::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::{Arc, Mutex};
use crate::table::memo::{MemoTableTypes, MemoTableWithTypes, MemoTableWithTypesMut};
use crate::{Id, IngredientIndex, Revision};

pub(crate) mod memo;

const PAGE_LEN_BITS: usize = 10;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;
const MAX_PAGES: usize = 1 << (u32::BITS as usize - PAGE_LEN_BITS);

/// A typed [`Page`] view.
pub(crate) struct PageView<'p, T: Slot>(&'p Page, PhantomData<&'p T>);

pub struct Table {
    pages: boxcar::Vec<Page>,
    /// Map from ingredient to non-full pages that are up for grabs
    non_full_pages: Mutex<FxHashMap<IngredientIndex, Vec<PageIndex>>>,
}

pub(crate) trait Slot: Any + Send + Sync {
    /// Access the [`MemoTable`][] for this slot.
    ///
    /// # Safety condition
    ///
    /// The current revision MUST be the current revision of the database containing this slot.
    unsafe fn memos(slot: *const Self, current_revision: Revision) -> *const MemoTable;

    /// Mutably access the [`MemoTable`] for this slot.
    fn memos_mut(&mut self) -> &mut MemoTable;
}

/// [Slot::memos]
type SlotMemosFnErased = unsafe fn(*const (), current_revision: Revision) -> *const MemoTable;
/// [Slot::memos]
type SlotMemosFn<T> = unsafe fn(*const T, current_revision: Revision) -> *const MemoTable;
/// [Slot::memos_mut]
type SlotMemosMutFnErased = unsafe fn(*mut ()) -> *mut MemoTable;
/// [Slot::memos_mut]
type SlotMemosMutFn<T> = fn(&mut T) -> &mut MemoTable;

struct SlotVTable {
    layout: Layout,
    /// [`Slot`] methods
    memos: SlotMemosFnErased,
    memos_mut: SlotMemosMutFnErased,
    /// A drop impl to call when the own page drops
    /// SAFETY: The caller is required to supply a correct data pointer to a `Box<PageDataEntry<T>>` and initialized length,
    /// and correct memo types.
    drop_impl: unsafe fn(data: *mut (), initialized: usize, memo_types: &MemoTableTypes),
}

impl SlotVTable {
    const fn of<T: Slot>() -> &'static Self {
        const {
            &Self {
                drop_impl: |data, initialized, memo_types|
                // SAFETY: The caller is required to supply a correct data pointer and initialized length
                unsafe {
                    let data = Box::from_raw(data.cast::<PageData<T>>());
                    for i in 0..initialized {
                        let item = data[i].get().cast::<T>();
                        memo_types.attach_memos_mut((*item).memos_mut()).drop();
                        ptr::drop_in_place(item);
                    }
                },
                layout: Layout::new::<T>(),
                // SAFETY: The signatures are compatible
                memos: unsafe { mem::transmute::<SlotMemosFn<T>, SlotMemosFnErased>(T::memos) },
                // SAFETY: The signatures are compatible
                memos_mut: unsafe {
                    mem::transmute::<SlotMemosMutFn<T>, SlotMemosMutFnErased>(T::memos_mut)
                },
            }
        }
    }
}

type PageDataEntry<T> = UnsafeCell<MaybeUninit<T>>;
type PageData<T> = [PageDataEntry<T>; PAGE_LEN];

struct Page {
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
    /// This is a box allocated `PageData<SlotType>`
    data: NonNull<()>,

    /// A vtable for the slot type stored in this page.
    slot_vtable: &'static SlotVTable,
    /// The type id of what is stored as entries in data.
    // FIXME: Move this into SlotVTable once const stable
    slot_type_id: TypeId,
    /// The type name of what is stored as entries in data.
    // FIXME: Move this into SlotVTable once const stable
    slot_type_name: &'static str,

    memo_types: Arc<MemoTableTypes>,
}

// SAFETY: `Page` is `Send` as we make sure to only ever store `Slot` types in it which
// requires `Send`.`
unsafe impl Send for Page /* where for<M: Memo> M: Send */ {}
// SAFETY: `Page` is `Sync` as we make sure to only ever store `Slot` types in it which
// requires `Sync`.`
unsafe impl Sync for Page /* where for<M: Memo> M: Sync */ {}

#[derive(Copy, Clone, Debug)]
pub struct PageIndex(usize);

impl PageIndex {
    #[inline]
    fn new(idx: usize) -> Self {
        debug_assert!(idx < MAX_PAGES);
        Self(idx)
    }
}

#[derive(Copy, Clone, Debug)]
struct SlotIndex(usize);

impl SlotIndex {
    #[inline]
    fn new(idx: usize) -> Self {
        debug_assert!(idx < PAGE_LEN);
        Self(idx)
    }
}

impl Default for Table {
    fn default() -> Self {
        Self {
            pages: boxcar::Vec::new(),
            non_full_pages: Default::default(),
        }
    }
}

impl Table {
    /// Returns the [`IngredientIndex`] for an [`Id`].
    #[inline]
    pub fn ingredient_index(&self, id: Id) -> IngredientIndex {
        let (page_idx, _) = split_id(id);
        self.pages[page_idx.0].ingredient
    }

    /// Get a reference to the data for `id`, which must have been allocated from this table with type `T`.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    pub(crate) fn get<T: Slot>(&self, id: Id) -> &T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        &page_ref.data()[slot.0]
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
    pub(crate) fn get_raw<T: Slot>(&self, id: Id) -> *mut T {
        let (page, slot) = split_id(id);
        let page_ref = self.page::<T>(page);
        page_ref.page_data()[slot.0].get().cast::<T>()
    }

    /// Gets a reference to the page which has slots of type `T`
    ///
    /// # Panics
    ///
    /// If `page` is out of bounds or the type `T` is incorrect.
    #[inline]
    pub(crate) fn page<T: Slot>(&self, page: PageIndex) -> PageView<'_, T> {
        self.pages[page.0].assert_type::<T>()
    }

    /// Allocate a new page for the given ingredient and with slots of type `T`
    #[inline]
    pub(crate) fn push_page<T: Slot>(
        &self,
        ingredient: IngredientIndex,
        memo_types: Arc<MemoTableTypes>,
    ) -> PageIndex {
        PageIndex::new(self.pages.push(Page::new::<T>(ingredient, memo_types)))
    }

    /// Get the memo table associated with `id`
    ///
    /// # Safety condition
    ///
    /// The parameter `current_revision` MUST be the current revision
    /// of the owner of database owning this table.
    pub(crate) unsafe fn memos(
        &self,
        id: Id,
        current_revision: Revision,
    ) -> MemoTableWithTypes<'_> {
        let (page, slot) = split_id(id);
        let page = &self.pages[page.0];
        // SAFETY: We supply a proper slot pointer and the caller is required to pass the `current_revision`.
        let memos = unsafe { &*(page.slot_vtable.memos)(page.get(slot), current_revision) };
        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.memo_types.attach_memos(memos) }
    }

    /// Get the memo table associated with `id`
    pub(crate) fn memos_mut(&mut self, id: Id) -> MemoTableWithTypesMut<'_> {
        let (page, slot) = split_id(id);
        let page_index = page.0;
        let page = self
            .pages
            .get_mut(page_index)
            .unwrap_or_else(|| panic!("index `{page_index}` is uninitialized"));
        // SAFETY: We supply a proper slot pointer and the caller is required to pass the `current_revision`.
        let memos = unsafe { &mut *(page.slot_vtable.memos_mut)(page.get(slot)) };
        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.memo_types.attach_memos_mut(memos) }
    }

    pub(crate) fn slots_of<T: Slot>(&self) -> impl Iterator<Item = &T> + '_ {
        self.pages
            .iter()
            .filter_map(|(_, page)| page.cast_type::<T>())
            .flat_map(|view| view.data())
    }

    pub(crate) fn fetch_or_push_page<T: Slot>(
        &self,
        ingredient: IngredientIndex,
        memo_types: impl FnOnce() -> Arc<MemoTableTypes>,
    ) -> PageIndex {
        if let Some(page) = self
            .non_full_pages
            .lock()
            .get_mut(&ingredient)
            .and_then(Vec::pop)
        {
            return page;
        }
        self.push_page::<T>(ingredient, memo_types())
    }

    pub(crate) fn record_unfilled_page(&self, ingredient: IngredientIndex, page: PageIndex) {
        self.non_full_pages
            .lock()
            .entry(ingredient)
            .or_default()
            .push(page);
    }
}

impl<'p, T: Slot> PageView<'p, T> {
    #[inline]
    fn page_data(&self) -> &'p [PageDataEntry<T>] {
        let len = self.0.allocated.load(Ordering::Acquire);
        // SAFETY: `len` is the initialized length of the page
        unsafe { slice::from_raw_parts(self.0.data.cast::<PageDataEntry<T>>().as_ptr(), len) }
    }

    #[inline]
    fn data(&self) -> &'p [T] {
        let len = self.0.allocated.load(Ordering::Acquire);
        // SAFETY: `len` is the initialized length of the page
        unsafe { slice::from_raw_parts(self.0.data.cast::<T>().as_ptr(), len) }
    }

    pub(crate) fn allocate<V>(&self, page: PageIndex, value: V) -> Result<Id, V>
    where
        V: FnOnce(Id) -> T,
    {
        let _guard = self.0.allocation_lock.lock();
        let index = self.0.allocated.load(Ordering::Acquire);
        if index >= PAGE_LEN {
            return Err(value);
        }

        // Initialize entry `index`
        let id = make_id(page, SlotIndex::new(index));
        let data = self.0.data.cast::<PageDataEntry<T>>();

        // SAFETY: `index` is also guaranteed to be in bounds as per the check above.
        let entry = unsafe { &*data.as_ptr().add(index) };

        // SAFETY: We acquired the allocation lock, so we have unique access to the UnsafeCell
        // interior
        unsafe { (*entry.get()).write(value(id)) };

        // Update the length (this must be done after initialization as otherwise an uninitialized
        // read could occur!)
        self.0.allocated.store(index + 1, Ordering::Release);

        Ok(id)
    }
}

impl Page {
    #[inline]
    fn new<T: Slot>(ingredient: IngredientIndex, memo_types: Arc<MemoTableTypes>) -> Self {
        #[cfg(not(feature = "shuttle"))]
        let data: Box<PageData<T>> =
            Box::new([const { UnsafeCell::new(MaybeUninit::uninit()) }; PAGE_LEN]);

        #[cfg(feature = "shuttle")]
        let data = {
            // Avoid stack overflows when using larger shuttle types.
            let data = (0..PAGE_LEN)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect::<Box<[PageDataEntry<T>]>>();

            let data: *mut [PageDataEntry<T>] = Box::into_raw(data);

            // SAFETY: `*mut PageDataEntry<T>` and `*mut [PageDataEntry<T>; N]` have the same layout.
            unsafe { Box::from_raw(data.cast::<PageDataEntry<T>>().cast::<PageData<T>>()) }
        };

        Self {
            slot_vtable: SlotVTable::of::<T>(),
            slot_type_id: TypeId::of::<T>(),
            slot_type_name: std::any::type_name::<T>(),
            ingredient,
            allocated: Default::default(),
            allocation_lock: Default::default(),
            data: NonNull::from(Box::leak(data)).cast::<()>(),
            memo_types,
        }
    }

    /// Retrieves the pointer for the given slot.
    ///
    /// # Panics
    ///
    /// If slot is out of bounds
    fn get(&self, slot: SlotIndex) -> *mut () {
        let len = self.allocated.load(Ordering::Acquire);
        assert!(
            slot.0 < len,
            "out of bounds access `{slot:?}` (maximum slot `{len}`)"
        );
        // SAFETY: We have checked that the resulting pointer will be within bounds.
        unsafe {
            self.data
                .as_ptr()
                .byte_add(slot.0 * self.slot_vtable.layout.size())
        }
    }

    #[inline]
    fn assert_type<T: Slot>(&self) -> PageView<T> {
        assert_eq!(
            self.slot_type_id,
            TypeId::of::<T>(),
            "page has slot type `{:?}` but `{:?}` was expected",
            self.slot_type_name,
            std::any::type_name::<T>(),
        );
        PageView(self, PhantomData)
    }

    fn cast_type<T: Slot>(&self) -> Option<PageView<T>> {
        if self.slot_type_id == TypeId::of::<T>() {
            Some(PageView(self, PhantomData))
        } else {
            None
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let len = *self.allocated.get_mut();
        // SAFETY: We supply the data pointer and the initialized length
        unsafe { (self.slot_vtable.drop_impl)(self.data.as_ptr(), len, &self.memo_types) };
    }
}

fn make_id(page: PageIndex, slot: SlotIndex) -> Id {
    let page = page.0 as u32;
    let slot = slot.0 as u32;
    // SAFETY: `slot` is guaranteed to be small enough that the resulting Id won't be bigger than `Id::MAX_U32`
    unsafe { Id::from_index((page << PAGE_LEN_BITS) | slot) }
}

#[inline]
fn split_id(id: Id) -> (PageIndex, SlotIndex) {
    let index = id.index() as usize;
    let slot = index & PAGE_LEN_MASK;
    let page = index >> PAGE_LEN_BITS;
    (PageIndex::new(page), SlotIndex::new(slot))
}
