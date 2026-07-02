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

const PAGE_CLASS_SHIFT: usize = 30;
const PAGE_CLASS_MASK: usize = 3 << PAGE_CLASS_SHIFT;
const PAGE_CLASS_LEN: usize = 1 << PAGE_CLASS_SHIFT;
const PAGE_CLASS_COUNT: usize = 4;

mod sealed {
    pub trait Sealed {
        const CAPACITY: usize;
    }
}

/// Type-level table page-size policy used by Salsa's generated code.
#[doc(hidden)]
pub trait PageSize: sealed::Sealed + Send + Sync + 'static {}

#[doc(hidden)]
pub struct PageSizeConst<const N: usize>;

impl<const N: usize> sealed::Sealed for PageSizeConst<N> {
    const CAPACITY: usize = N;
}

impl PageSize for PageSizeConst<128> {}
impl PageSize for PageSizeConst<256> {}
impl PageSize for PageSizeConst<512> {}
impl PageSize for PageSizeConst<1024> {}

#[doc(hidden)]
pub type PageSize128 = PageSizeConst<128>;
#[doc(hidden)]
pub type PageSize256 = PageSizeConst<256>;
#[doc(hidden)]
pub type PageSize512 = PageSizeConst<512>;
#[doc(hidden)]
pub type PageSize1024 = PageSizeConst<1024>;

#[inline]
pub(crate) const fn page_capacity<P: PageSize>() -> usize {
    <P as sealed::Sealed>::CAPACITY
}

#[inline]
const fn page_class<P: PageSize>() -> usize {
    10 - page_capacity::<P>().trailing_zeros() as usize
}

const fn class_capacity(class: usize) -> usize {
    1024 >> class
}

/// A typed [`Page`] view.
pub(crate) struct PageView<'p, T: Slot>(&'p Page, PhantomData<&'p T>);

pub struct Table {
    pages: [boxcar::Vec<Page>; PAGE_CLASS_COUNT],
    /// Map from ingredient to non-full pages that are up for grabs
    non_full_pages: Mutex<FxHashMap<IngredientIndex, Vec<PageIndex>>>,
}

/// # Safety
///
/// Implementors of this trait need to make sure that their type is unique with respect to
/// their owning ingredient as the allocation strategy relies on this.
pub unsafe trait Slot: Any + Send + Sync {
    type PageSize: PageSize;

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
    /// The type name of what is stored as entries in data.
    type_name: fn() -> &'static str,
    /// Drops the page allocation and its initialized prefix.
    /// SAFETY: The pointer, slot type, initialized length, and memo types must match.
    drop_impl: unsafe fn(data: *mut (), initialized: usize, memo_types: &MemoTableTypes),
}

impl SlotVTable {
    const fn of<T: Slot>() -> &'static Self {
        const {
            &Self {
                drop_impl: |data, initialized, memo_types| {
                    // SAFETY: `Page` supplies the allocation and initialized length belonging to `T`.
                    unsafe { drop_page::<T>(NonNull::new_unchecked(data), initialized, memo_types) }
                },
                layout: Layout::new::<T>(),
                type_name: std::any::type_name::<T>,
                // SAFETY: The signatures are ABI-compatible.
                memos: unsafe { mem::transmute::<SlotMemosFn<T>, SlotMemosFnErased>(T::memos) },
                // SAFETY: The signatures are ABI-compatible.
                memos_mut: unsafe {
                    mem::transmute::<SlotMemosMutFn<T>, SlotMemosMutFnErased>(T::memos_mut)
                },
            }
        }
    }
}

type PageDataEntry<T> = UnsafeCell<MaybeUninit<T>>;

fn allocate_page<T: Slot>() -> NonNull<()> {
    // A boxed slice avoids materializing a large page on the stack.
    let data = (0..page_capacity::<T::PageSize>())
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect::<Box<[PageDataEntry<T>]>>();
    let data = Box::into_raw(data) as *mut PageDataEntry<T>;
    NonNull::new(data).unwrap().cast()
}

unsafe fn drop_page<T: Slot>(data: NonNull<()>, initialized: usize, memo_types: &MemoTableTypes) {
    let data = ptr::slice_from_raw_parts_mut(
        data.cast::<PageDataEntry<T>>().as_ptr(),
        page_capacity::<T::PageSize>(),
    );
    // SAFETY: `Page::new` allocated this slice for `T` and its page policy.
    let data = unsafe { Box::from_raw(data) };
    for entry in &data[..initialized] {
        let item = entry.get().cast::<T>();
        // SAFETY: `allocated` tracks the initialized prefix.
        unsafe {
            memo_types.attach_memos_mut((*item).memos_mut()).drop();
            ptr::drop_in_place(item);
        }
    }
}

struct Page {
    /// The ingredient for elements on this page.
    ingredient: IngredientIndex,

    /// Number of elements of `data` that are initialized.
    allocated: AtomicUsize,

    /// The potentially uninitialized data of this page. As we initialize new entries, we increment `allocated`.
    /// This is a box allocated `PageData<SlotType>`
    data: NonNull<()>,

    /// A vtable for the slot type stored in this page.
    slot_vtable: &'static SlotVTable,
    /// The type id of what is stored as entries in data.
    // FIXME: Move this into SlotVTable once const stable
    slot_type_id: TypeId,

    memo_types: Arc<MemoTableTypes>,
}

// SAFETY: `Page` is `Send` as we make sure to only ever store `Slot` types in it which
// requires `Send`.`
unsafe impl Send for Page /* where for<M: Memo> M: Send */ {}
// SAFETY: `Page` is `Sync` as we make sure to only ever store `Slot` types in it which
// requires `Sync`.`
unsafe impl Sync for Page /* where for<M: Memo> M: Sync */ {}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageIndex {
    class: usize,
    index: usize,
}

impl PageIndex {
    #[inline]
    fn new<P: PageSize>(index: usize) -> Self {
        let class = page_class::<P>();
        let tag = class << PAGE_CLASS_SHIFT;
        let limit = (tag + PAGE_CLASS_LEN).min(Id::MAX_USIZE);
        assert!(index < (limit - tag) / page_capacity::<P>());
        Self { class, index }
    }

    #[allow(dead_code)]
    pub fn as_usize(&self) -> usize {
        self.index
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SlotIndex(usize);

impl SlotIndex {
    #[inline]
    fn new<P: PageSize>(idx: usize) -> Self {
        debug_assert!(idx < page_capacity::<P>());
        Self(idx)
    }
}

impl Default for Table {
    fn default() -> Self {
        Self {
            pages: std::array::from_fn(|_| boxcar::Vec::new()),
            non_full_pages: Default::default(),
        }
    }
}

impl Table {
    /// Returns the [`IngredientIndex`] for an [`Id`].
    #[inline]
    pub fn ingredient_index(&self, id: Id) -> IngredientIndex {
        let (page, _) = split_erased_id(id);
        self.pages[page.class][page.index].ingredient
    }

    /// Get a reference to the data for `id`, which must have been allocated from this table with type `T`.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    pub(crate) fn get<T: Slot>(&self, id: Id) -> &T {
        let (page, slot) = split_typed_id::<T::PageSize>(id);
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
        let (page, slot) = split_typed_id::<T::PageSize>(id);
        let page_ref = self.page::<T>(page);
        page_ref.page_data()[slot.0].get().cast::<T>()
    }

    /// Returns the number of pages that have been allocated.
    pub fn page_count(&self) -> usize {
        self.pages.iter().map(boxcar::Vec::count).sum()
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn page_infos(&self) -> FxHashMap<IngredientIndex, crate::database::PageInfo> {
        let mut page_fills: FxHashMap<IngredientIndex, (usize, Vec<usize>)> = FxHashMap::default();

        for (class, pages) in self.pages.iter().enumerate() {
            let capacity = class_capacity(class);
            for (_, page) in pages.iter() {
                let fill = page.allocated.load(Ordering::Acquire);
                if fill > 0 {
                    let (ingredient_capacity, fills) = page_fills
                        .entry(page.ingredient)
                        .or_insert_with(|| (capacity, Vec::new()));
                    debug_assert_eq!(*ingredient_capacity, capacity);
                    fills.push(fill);
                }
            }
        }

        page_fills
            .into_iter()
            .filter_map(|(ingredient, (capacity, fills))| {
                crate::database::PageInfo::from_page_fills(capacity, fills)
                    .map(|page_info| (ingredient, page_info))
            })
            .collect()
    }

    /// Gets a reference to the page which has slots of type `T`
    ///
    /// # Panics
    ///
    /// If `page` is out of bounds or the type `T` is incorrect.
    #[inline]
    pub(crate) fn page<T: Slot>(&self, page: PageIndex) -> PageView<'_, T> {
        debug_assert_eq!(page.class, page_class::<T::PageSize>());
        self.pages[page.class][page.index].assert_type::<T>()
    }

    /// Force initialize the page at the given index.
    ///
    /// If the page at the provided index was created using `push_uninit_page`, it
    /// will be initialized using the provided ingredient data.
    ///
    /// Otherwise, the page will be allocated.
    ///
    /// # Panics
    ///
    /// If `page` is out of bounds or the type `T` is incorrect.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn force_page<T: Slot>(
        &mut self,
        page_idx: PageIndex,
        ingredient: IngredientIndex,
        memo_types: &Arc<MemoTableTypes>,
    ) {
        let class = page_class::<T::PageSize>();
        assert_eq!(page_idx.class, class);
        let page = self.pages[class].get_mut(page_idx.index);

        match page {
            Some(page) => {
                // Initialize the page if was created using `push_uninit_page`.
                if page.slot_type_id == TypeId::of::<DummySlot<T::PageSize>>() {
                    *page = Page::new::<T>(ingredient, memo_types.clone());
                }

                // Ensure the page has the correct type.
                page.assert_type::<T>();
            }

            None => {
                // Create dummy pages until we reach the page we want.
                while self.pages[class].count() < page_idx.as_usize() {
                    // We make sure not to claim any intermediary pages for ourselves, as they may
                    // be required by a different ingredient when it is deserialized.
                    self.push_uninit_page::<T::PageSize>();
                }

                let allocated_idx = self.push_page::<T>(ingredient, memo_types.clone());
                assert_eq!(
                    allocated_idx, page_idx,
                    "allocated index does not match requested index"
                );
            }
        };
    }

    /// Allocate a new page for the given ingredient and with slots of type `T`
    #[inline]
    pub(crate) fn push_page<T: Slot>(
        &self,
        ingredient: IngredientIndex,
        memo_types: Arc<MemoTableTypes>,
    ) -> PageIndex {
        let class = page_class::<T::PageSize>();
        PageIndex::new::<T::PageSize>(
            self.pages[class].push(Page::new::<T>(ingredient, memo_types)),
        )
    }

    /// Allocate an uninitialized page.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn push_uninit_page<P: PageSize>(&self) -> PageIndex {
        // Note that `DummySlot` is a ZST, so the memory wasted by any pages of ingredients
        // that were not serialized should be negligible.
        let class = page_class::<P>();
        PageIndex::new::<P>(self.pages[class].push(Page::new::<DummySlot<P>>(
            IngredientIndex::new(0),
            Arc::new(MemoTableTypes::default()),
        )))
    }

    /// Get the memo table associated with `id` for the concrete type `T`.
    ///
    /// # Safety
    ///
    /// The parameter `current_revision` must be the current revision of the database
    /// owning this table.
    ///
    /// # Panics
    ///
    /// If `page` is out of bounds or the type `T` is incorrect.
    pub unsafe fn memos<T: Slot>(
        &self,
        id: Id,
        current_revision: Revision,
    ) -> MemoTableWithTypes<'_> {
        let (page, slot) = split_typed_id::<T::PageSize>(id);
        let page = self.pages[page.class][page.index].assert_type::<T>();
        let slot = &page.data()[slot.0];

        // SAFETY: The caller is required to pass the `current_revision`.
        let memos = unsafe { &*T::memos(slot, current_revision) };

        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.0.memo_types.attach_memos(memos) }
    }

    /// Get the memo table associated with `id`.
    ///
    /// Unlike `Table::memos`, this does not require a concrete type, and instead uses dynamic
    /// dispatch.
    ///
    /// # Safety
    ///
    /// The parameter `current_revision` must be the current revision of the owner of database
    /// owning this table.
    pub unsafe fn dyn_memos(&self, id: Id, current_revision: Revision) -> MemoTableWithTypes<'_> {
        let (page, slot) = split_erased_id(id);
        let page = &self.pages[page.class][page.index];
        // SAFETY: We supply a proper slot pointer and the caller is required to pass the `current_revision`.
        let memos = unsafe { &*(page.slot_vtable.memos)(page.get(slot), current_revision) };
        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.memo_types.attach_memos(memos) }
    }

    /// Get the memo table associated with `id`
    pub(crate) fn memos_mut(&mut self, id: Id) -> MemoTableWithTypesMut<'_> {
        let (page, slot) = split_erased_id(id);
        let page_index = page.index;
        let page = self.pages[page.class]
            .get_mut(page_index)
            .unwrap_or_else(|| panic!("index `{page_index}` is uninitialized"));
        // SAFETY: We supply a proper slot pointer and the caller is required to pass the `current_revision`.
        let memos = unsafe { &mut *(page.slot_vtable.memos_mut)(page.get(slot)) };
        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.memo_types.attach_memos_mut(memos) }
    }

    pub(crate) fn slots_of<T: Slot>(&self) -> impl Iterator<Item = (Id, &T)> + '_ {
        let class = page_class::<T::PageSize>();
        ErasedSlots {
            class,
            pages: self.pages[class].iter(),
            current_page: None,
            slot_type_id: TypeId::of::<T>(),
        }
        .map(|(id, slot)| {
            // SAFETY: `ErasedSlots` only visits pages whose `TypeId` matches `T` and only returns
            // pointers to initialized slots.
            (id, unsafe { &*slot.cast::<T>() })
        })
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn fetch_or_push_page<T: Slot>(
        &self,
        ingredient: IngredientIndex,
        memo_types: impl FnOnce() -> Arc<MemoTableTypes>,
    ) -> PageIndex {
        if let Some(page) = self.take_non_full_page(ingredient) {
            debug_assert_eq!(page.class, page_class::<T::PageSize>());
            return page;
        }

        self.push_page::<T>(ingredient, memo_types())
    }

    fn take_non_full_page(&self, ingredient: IngredientIndex) -> Option<PageIndex> {
        self.non_full_pages
            .lock()
            .get_mut(&ingredient)
            .and_then(Vec::pop)
    }

    pub(crate) fn record_unfilled_page(&self, ingredient: IngredientIndex, page: PageIndex) {
        self.non_full_pages
            .lock()
            .entry(ingredient)
            .or_default()
            .push(page);
    }
}

struct ErasedSlots<'db> {
    class: usize,
    pages: boxcar::vec::Iter<'db, Page>,
    current_page: Option<(PageIndex, &'db Page, std::ops::Range<usize>)>,
    slot_type_id: TypeId,
}

impl Iterator for ErasedSlots<'_> {
    type Item = (Id, *mut ());

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((page_index, page, slots)) = &mut self.current_page {
                if let Some(slot_index) = slots.next() {
                    let slot_index = SlotIndex(slot_index);
                    let id = make_erased_id(*page_index, slot_index);

                    // SAFETY: `slot_index` is below the initialized length captured when the page
                    // became current, so the resulting pointer is within the page allocation.
                    let slot = unsafe {
                        page.data
                            .as_ptr()
                            .byte_add(slot_index.0 * page.slot_vtable.layout.size())
                    };

                    return Some((id, slot));
                }
            }

            self.current_page = self.pages.find_map(|(page_index, page)| {
                (page.slot_type_id == self.slot_type_id).then(|| {
                    (
                        PageIndex {
                            class: self.class,
                            index: page_index,
                        },
                        page,
                        0..page.allocated.load(Ordering::Acquire),
                    )
                })
            });
            self.current_page.as_ref()?;
        }
    }
}

impl<'db, T: Slot> PageView<'db, T> {
    #[inline]
    fn page_data(&self) -> &'db [PageDataEntry<T>] {
        let len = self.0.allocated.load(Ordering::Acquire);
        // SAFETY: `len` is the initialized length of the page
        unsafe { slice::from_raw_parts(self.0.data.cast::<PageDataEntry<T>>().as_ptr(), len) }
    }

    #[inline]
    fn data(&self) -> &'db [T] {
        let len = self.0.allocated.load(Ordering::Acquire);
        // SAFETY: `len` is the initialized length of the page
        unsafe { slice::from_raw_parts(self.0.data.cast::<T>().as_ptr(), len) }
    }

    /// Allocate a value in this page.
    ///
    /// # Safety
    ///
    /// The caller must be the unique writer to this page, i.e. `allocate` cannot be called
    /// concurrently by multiple threads. Concurrent readers however, are fine.
    #[inline]
    pub(crate) unsafe fn allocate<V>(&self, page: PageIndex, value: V) -> Result<(Id, &'db T), V>
    where
        V: FnOnce(Id) -> T,
    {
        let index = self.0.allocated.load(Ordering::Acquire);
        if index >= page_capacity::<T::PageSize>() {
            return Err(value);
        }

        // Initialize entry `index`
        let id = make_id::<T::PageSize>(page, SlotIndex::new::<T::PageSize>(index));
        let data = self.0.data.cast::<PageDataEntry<T>>();

        // SAFETY: `index` is also guaranteed to be in bounds as per the check above.
        let entry = unsafe { &*data.as_ptr().add(index) };

        // SAFETY: The caller guarantees we are the unique writer, and readers will not attempt to
        // access this index until we have updated the length.
        unsafe { (*entry.get()).write(value(id)) };

        // SAFETY: We just initialized the value above.
        let value = unsafe { (*entry.get()).assume_init_ref() };

        // Update the length now that we have initialized the value.
        self.0.allocated.store(index + 1, Ordering::Release);

        Ok((id, value))
    }
}

impl Page {
    #[inline]
    fn new<T: Slot>(ingredient: IngredientIndex, memo_types: Arc<MemoTableTypes>) -> Self {
        Self {
            ingredient,
            memo_types,
            slot_vtable: SlotVTable::of::<T>(),
            slot_type_id: TypeId::of::<T>(),
            allocated: AtomicUsize::new(0),
            data: allocate_page::<T>(),
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
    fn assert_type<T: Slot>(&self) -> PageView<'_, T> {
        if self.slot_type_id != TypeId::of::<T>() {
            type_assert_failed::<T>(self);
        }

        PageView(self, PhantomData)
    }
}

/// This function is explicitly outlined to avoid debug machinery in the hot-path.
#[cold]
#[inline(never)]
fn type_assert_failed<T: 'static>(page: &Page) -> ! {
    panic!(
        "page has slot type `{:?}` but `{:?}` was expected",
        (page.slot_vtable.type_name)(),
        std::any::type_name::<T>(),
    )
}

impl Drop for Page {
    fn drop(&mut self) {
        let len = *self.allocated.get_mut();
        // SAFETY: We supply the data pointer and the initialized length
        unsafe { (self.slot_vtable.drop_impl)(self.data.as_ptr(), len, &self.memo_types) };
    }
}

/// A placeholder type representing the slots of an uninitialized `Page`.
struct DummySlot<P>(PhantomData<P>);

// SAFETY: The `DummySlot type is private.
unsafe impl<P: PageSize> Slot for DummySlot<P> {
    type PageSize = P;

    unsafe fn memos(_: *const Self, _: Revision) -> *const MemoTable {
        unreachable!()
    }

    fn memos_mut(&mut self) -> &mut MemoTable {
        unreachable!()
    }
}

fn make_id<P: PageSize>(page: PageIndex, slot: SlotIndex) -> Id {
    debug_assert_eq!(page.class, page_class::<P>());
    debug_assert!(slot.0 < page_capacity::<P>());
    make_erased_id(page, slot)
}

fn make_erased_id(page: PageIndex, slot: SlotIndex) -> Id {
    let capacity = class_capacity(page.class);
    debug_assert!(slot.0 < capacity);
    let index = (page.class << PAGE_CLASS_SHIFT) + page.index * capacity + slot.0;
    // SAFETY: `PageIndex::new` keeps the complete page below `Id::MAX_U32`.
    unsafe { Id::from_index(index as u32) }
}

#[inline]
fn split_erased_id(id: Id) -> (PageIndex, SlotIndex) {
    let class = id.index() as usize >> PAGE_CLASS_SHIFT;
    split_id(id, class, class_capacity(class))
}

#[inline]
fn split_typed_id<P: PageSize>(id: Id) -> (PageIndex, SlotIndex) {
    let class = page_class::<P>();
    debug_assert_eq!(id.index() as usize >> PAGE_CLASS_SHIFT, class);
    split_id(id, class, page_capacity::<P>())
}

#[inline]
fn split_id(id: Id, class: usize, capacity: usize) -> (PageIndex, SlotIndex) {
    let offset = id.index() as usize & !PAGE_CLASS_MASK;
    (
        PageIndex {
            class,
            index: offset / capacity,
        },
        SlotIndex(offset % capacity),
    )
}

#[cfg(feature = "persistence")]
pub(crate) fn try_split_typed_id<P: PageSize>(id: Id) -> Option<(PageIndex, SlotIndex)> {
    (id.index() < Id::MAX_U32 && id.index() as usize >> PAGE_CLASS_SHIFT == page_class::<P>())
        .then(|| split_typed_id::<P>(id))
}
