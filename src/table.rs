use std::alloc::Layout;
use std::any::{Any, TypeId};
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr::{self, NonNull};

use memo::MemoTable;
use rustc_hash::FxHashMap;

use crate::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::{Arc, Mutex};
use crate::table::memo::{MemoTableTypes, MemoTableWithTypes, MemoTableWithTypesMut};
use crate::{Id, IngredientIndex, Revision};

pub(crate) mod memo;

const PAGE_LEN_BITS: usize = 7;
const PAGE_LEN_MASK: usize = PAGE_LEN - 1;
const PAGE_LEN: usize = 1 << PAGE_LEN_BITS;
const MAX_PAGES: usize = Id::MAX_USIZE / PAGE_LEN;

/// A typed [`Page`] view.
pub(crate) struct PageView<'p, T: Slot>(&'p Page, PhantomData<&'p T>);

pub struct Table {
    pages: boxcar::Vec<Page>,
    /// Map from ingredient to non-full pages that are up for grabs
    non_full_pages: Mutex<FxHashMap<IngredientIndex, Vec<PageIndex>>>,
}

/// Pages populated by one ingredient while the other ingredients restore independently.
#[cfg(feature = "persistence")]
#[derive(Default)]
pub struct RestoredPages(FxHashMap<usize, Page>);

#[cfg(feature = "persistence")]
pub(crate) enum RestoreSlots<'a> {
    Table(&'a mut Table),
    Pages(&'a mut RestoredPages),
}

#[cfg(feature = "persistence")]
impl RestoreSlots<'_> {
    pub(crate) fn restore_slot<T: Slot>(
        &mut self,
        id: Id,
        ingredient: IngredientIndex,
        types: &Arc<MemoTableTypes>,
        value: T,
    ) -> Result<&T, &'static str> {
        match self {
            Self::Table(table) => table.restore_slot(id, ingredient, types, value),
            Self::Pages(pages) => {
                let (index, slot) = split_id(id);
                let page = pages
                    .0
                    .entry(index.0)
                    .or_insert_with(|| Page::new::<T>(ingredient, types.clone()));
                page.restore_slot(slot, ingredient, value)
            }
        }
    }
}

/// # Safety
///
/// Implementors of this trait need to make sure that their type is unique with respect to
/// their owning ingredient as the allocation strategy relies on this.
pub unsafe trait Slot: Any + Send + Sync {
    /// Access the [`MemoTable`][] for this slot.
    ///
    /// # Safety condition
    ///
    /// The current revision MUST be the current revision of the database containing this slot.
    unsafe fn memos(slot: *const Self, current_revision: Revision) -> *const MemoTable;

    /// Access the memo table without validating or locking the slot.
    ///
    /// # Safety
    ///
    /// `slot` must be initialized and remain alive without reuse or destruction of its memo
    /// storage for the entire lifetime of any references obtained through the returned pointer.
    unsafe fn memos_unchecked(slot: *const Self) -> *const MemoTable;

    /// Mutably access the [`MemoTable`] for this slot.
    fn memos_mut(&mut self) -> &mut MemoTable;
}

/// [Slot::memos]
type SlotMemosFnErased = unsafe fn(*const (), current_revision: Revision) -> *const MemoTable;
/// [Slot::memos]
type SlotMemosFn<T> = unsafe fn(*const T, current_revision: Revision) -> *const MemoTable;
/// [Slot::memos_unchecked]
type SlotMemosUncheckedFnErased = unsafe fn(*const ()) -> *const MemoTable;
/// [Slot::memos_unchecked]
type SlotMemosUncheckedFn<T> = unsafe fn(*const T) -> *const MemoTable;
/// [Slot::memos_mut]
type SlotMemosMutFnErased = unsafe fn(*mut ()) -> *mut MemoTable;
/// [Slot::memos_mut]
type SlotMemosMutFn<T> = fn(&mut T) -> &mut MemoTable;

struct SlotVTable {
    layout: Layout,
    /// [`Slot`] methods
    memos: SlotMemosFnErased,
    memos_unchecked: SlotMemosUncheckedFnErased,
    memos_mut: SlotMemosMutFnErased,
    /// The type name of what is stored as entries in data.
    type_name: fn() -> &'static str,
    /// Drops the initialized slots and frees the page's backing allocation.
    /// The caller must supply the page for this vtable with exclusive ownership.
    drop_impl: unsafe fn(&Page),
}

impl SlotVTable {
    const fn of<T: Slot>() -> &'static Self {
        const {
            &Self {
                drop_impl: |page| {
                    // SAFETY: The caller supplied the page allocated for T and owns its data.
                    let data = unsafe { Box::from_raw(page.data.as_ptr().cast::<PageData<T>>()) };
                    let end = page.allocated.load(Ordering::Relaxed);
                    for (i, entry) in data[..end].iter().enumerate() {
                        if page.is_hole(i) {
                            continue;
                        }
                        let item = entry.get().cast::<T>();
                        // SAFETY: Every slot below end except a restored hole is initialized.
                        unsafe {
                            page.memo_types.attach_memos_mut((*item).memos_mut()).drop();
                            ptr::drop_in_place(item);
                        }
                    }
                },
                layout: Layout::new::<T>(),
                type_name: std::any::type_name::<T>,
                // SAFETY: The signatures are ABI-compatible.
                memos: unsafe { mem::transmute::<SlotMemosFn<T>, SlotMemosFnErased>(T::memos) },
                // SAFETY: The signatures are ABI-compatible.
                memos_unchecked: unsafe {
                    mem::transmute::<SlotMemosUncheckedFn<T>, SlotMemosUncheckedFnErased>(
                        T::memos_unchecked,
                    )
                },
                // SAFETY: The signatures are ABI-compatible.
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

    /// End of the allocated prefix, including holes left by a rooted snapshot.
    allocated: AtomicUsize,

    /// Holes are only written while restoring a new, exclusively owned table.
    #[cfg(feature = "persistence")]
    holes: u128,

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
pub struct PageIndex(usize);

impl PageIndex {
    #[inline]
    fn new(idx: usize) -> Self {
        debug_assert!(idx < MAX_PAGES);
        Self(idx)
    }

    #[allow(dead_code)]
    pub fn as_usize(&self) -> usize {
        self.0
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SlotIndex(usize);

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

    #[cfg(feature = "persistence")]
    pub(crate) fn allocated_ingredient(&self, id: Id) -> Option<IngredientIndex> {
        let (page, slot) = split_id(id);
        let page = self.pages.get(page.0)?;
        (slot.0 < page.allocated.load(Ordering::Acquire) && !page.is_hole(slot.0))
            .then_some(page.ingredient)
    }

    /// Get a reference to the data for `id`, which must have been allocated from this table with type `T`.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    pub(crate) fn get<T: Slot>(&self, id: Id) -> &T {
        // SAFETY: get_raw checks the slot type, bounds, and initialization.
        unsafe { &*self.get_raw::<T>(id) }
    }

    /// Get a raw pointer to the data for `id`, which must have been allocated from this table.
    ///
    /// # Panics
    ///
    /// If `id` is out of bounds or the does not have the type `T`.
    ///
    /// # Safety
    ///
    /// Dereferencing the returned pointer must respect the aliasing rules for the slot.
    pub(crate) fn get_raw<T: Slot>(&self, id: Id) -> *mut T {
        let (page, slot) = split_id(id);
        self.page::<T>(page).get_raw(slot)
    }

    /// Returns the number of pages that have been allocated.
    pub fn page_count(&self) -> usize {
        self.pages.count()
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn page_capacity(&self) -> usize {
        PAGE_LEN
    }

    #[cfg(feature = "salsa_unstable")]
    pub(crate) fn page_infos(&self) -> FxHashMap<IngredientIndex, crate::database::PageInfo> {
        let mut page_fills: FxHashMap<IngredientIndex, Vec<usize>> = FxHashMap::default();

        for (_, page) in self.pages.iter() {
            let fill = page.allocated.load(Ordering::Acquire);
            #[cfg(feature = "persistence")]
            let fill = fill - page.holes.count_ones() as usize;
            if fill > 0 {
                page_fills.entry(page.ingredient).or_default().push(fill);
            }
        }

        page_fills
            .into_iter()
            .filter_map(|(ingredient, fills)| {
                crate::database::PageInfo::from_page_fills(PAGE_LEN, fills)
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
        self.pages[page.0].assert_type::<T>()
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
        let page = self.pages.get_mut(page_idx.0);

        match page {
            Some(page) => {
                // Initialize the page if was created using `push_uninit_page`.
                if page.slot_type_id == TypeId::of::<DummySlot>() {
                    *page = Page::new::<T>(ingredient, memo_types.clone());
                }

                // Ensure the page has the correct type.
                page.assert_type::<T>();
            }

            None => {
                // Create dummy pages until we reach the page we want.
                while self.page_count() < page_idx.as_usize() {
                    // We make sure not to claim any intermediary pages for ourselves, as they may
                    // be required by a different ingredient when it is deserialized.
                    self.push_uninit_page();
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
        PageIndex::new(self.pages.push(Page::new::<T>(ingredient, memo_types)))
    }

    /// Adopt pages whose backing allocations already contain the restored slots.
    /// Moving a page header preserves references from an ingredient to its interned values.
    #[cfg(feature = "persistence")]
    pub(crate) fn install_cache_pages(&mut self, pages: RestoredPages) -> Result<(), &'static str> {
        for (index, page) in pages.0 {
            if let Some(existing) = self.pages.get_mut(index) {
                if existing.slot_type_id != TypeId::of::<DummySlot>() {
                    return Err("different Salsa ingredients share a cache page");
                }
                *existing = page;
            } else {
                while self.page_count() < index {
                    self.push_uninit_page();
                }
                self.pages.push(page);
            }
        }
        Ok(())
    }

    /// Installs a restored value at its original address in the ID space. Slots between
    /// retained values can remain uninitialized when their objects were unreachable.
    #[cfg(feature = "persistence")]
    pub(crate) fn restore_slot<T: Slot>(
        &mut self,
        id: Id,
        ingredient: IngredientIndex,
        types: &Arc<MemoTableTypes>,
        value: T,
    ) -> Result<&T, &'static str> {
        let (page_idx, slot) = split_id(id);
        self.force_page::<T>(page_idx, ingredient, types);
        let page = self.pages.get_mut(page_idx.0).unwrap();
        page.restore_slot(slot, ingredient, value)
    }

    /// Allocate an uninitialized page.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn push_uninit_page(&self) -> PageIndex {
        // Note that `DummySlot` is a ZST, so the memory wasted by any pages of ingredients
        // that were not serialized should be negligible.
        PageIndex::new(self.pages.push(Page::new::<DummySlot>(
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
        let (page, slot) = split_id(id);
        let page = self.pages[page.0].assert_type::<T>();
        let slot = page.get_raw(slot);

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
        let (page, slot) = split_id(id);
        let page = &self.pages[page.0];
        // SAFETY: We supply a proper slot pointer and the caller is required to pass the `current_revision`.
        let memos = unsafe { &*(page.slot_vtable.memos)(page.get(slot), current_revision) };
        // SAFETY: The `Page` keeps the correct memo types.
        unsafe { page.memo_types.attach_memos(memos) }
    }

    /// Get the memo table without validating or locking its slot.
    ///
    /// # Safety
    ///
    /// The slot must remain alive without reuse or destruction of its memo storage for the
    /// entire lifetime of the returned table and any references obtained from it.
    pub(crate) unsafe fn dyn_memos_unchecked(&self, id: Id) -> MemoTableWithTypes<'_> {
        let (page, slot) = split_id(id);
        let page = &self.pages[page.0];
        // SAFETY: `Page::get` checks initialization, and the caller prevents reuse and destruction.
        let memos = unsafe { &*(page.slot_vtable.memos_unchecked)(page.get(slot)) };
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

    pub(crate) fn slots_of<T: Slot>(&self) -> impl Iterator<Item = (Id, &T)> + '_ {
        ErasedSlots {
            pages: self.pages.iter(),
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
    pages: boxcar::vec::Iter<'db, Page>,
    current_page: Option<(PageIndex, &'db Page, std::ops::Range<usize>)>,
    slot_type_id: TypeId,
}

impl Iterator for ErasedSlots<'_> {
    type Item = (Id, *mut ());

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((page_index, page, slots)) = &mut self.current_page
                && let Some(slot_index) = slots.next()
            {
                if page.is_hole(slot_index) {
                    continue;
                }
                let slot_index = SlotIndex::new(slot_index);
                let id = make_id(*page_index, slot_index);

                // SAFETY: `slot_index` is below the initialized length captured when the page
                // became current, so the resulting pointer is within the page allocation.
                let slot = unsafe {
                    page.data
                        .as_ptr()
                        .byte_add(slot_index.0 * page.slot_vtable.layout.size())
                };

                return Some((id, slot));
            }

            self.current_page = self.pages.find_map(|(page_index, page)| {
                (page.slot_type_id == self.slot_type_id).then(|| {
                    (
                        PageIndex::new(page_index),
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
    fn get_raw(&self, slot: SlotIndex) -> *mut T {
        #[cfg(feature = "persistence")]
        assert!(!self.0.is_hole(slot.0), "uninitialized slot {slot:?}");
        let len = self.0.allocated.load(Ordering::Acquire);
        // SAFETY: The page holds T and len is its allocated prefix. The entry
        // type permits uninitialized slots in sparse, restored pages.
        let entries = unsafe {
            std::slice::from_raw_parts(self.0.data.cast::<PageDataEntry<T>>().as_ptr(), len)
        };
        entries[slot.0].get().cast::<T>()
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
        if index >= PAGE_LEN {
            return Err(value);
        }

        // Initialize entry `index`
        let id = make_id(page, SlotIndex::new(index));
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
    #[cfg(feature = "persistence")]
    fn restore_slot<T: Slot>(
        &mut self,
        slot: SlotIndex,
        ingredient: IngredientIndex,
        value: T,
    ) -> Result<&T, &'static str> {
        if self.slot_type_id != TypeId::of::<T>() || self.ingredient != ingredient {
            return Err("different Salsa ingredients share a cache page");
        }
        let end = *self.allocated.get_mut();
        if slot.0 < end && !self.is_hole(slot.0) {
            return Err("duplicate slot in Salsa cache");
        }
        if slot.0 > end {
            // end and slot are both strictly below PAGE_LEN in this branch.
            self.holes |= ((1u128 << slot.0) - 1) & !((1u128 << end) - 1);
        }
        // SAFETY: The page type was checked above. The slot is in bounds and
        // uninitialized, and we exclusively own the table while installing it.
        let restored = unsafe { &mut *self.data.cast::<PageDataEntry<T>>().as_ptr().add(slot.0) };
        let restored = restored.get_mut().write(value);
        self.holes &= !(1u128 << slot.0);
        *self.allocated.get_mut() = end.max(slot.0 + 1);
        Ok(restored)
    }

    #[inline]
    fn is_hole(&self, slot: usize) -> bool {
        #[cfg(feature = "persistence")]
        {
            const { assert!(PAGE_LEN <= u128::BITS as usize) };
            self.holes != 0 && self.holes & (1 << slot) != 0
        }
        #[cfg(not(feature = "persistence"))]
        {
            let _ = slot;
            false
        }
    }

    #[inline]
    fn new<T: Slot>(ingredient: IngredientIndex, memo_types: Arc<MemoTableTypes>) -> Self {
        #[cfg(not(feature = "shuttle"))]
        let data: Box<PageData<T>> =
            Box::new([const { UnsafeCell::new(MaybeUninit::uninit()) }; PAGE_LEN]);

        // Avoid stack overflows when using larger Shuttle types.
        #[cfg(feature = "shuttle")]
        let data: Box<PageData<T>> = (0..PAGE_LEN)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Box<[_]>>()
            .try_into()
            .expect("collected exactly PAGE_LEN entries");

        Self {
            ingredient,
            memo_types,
            slot_vtable: SlotVTable::of::<T>(),
            slot_type_id: TypeId::of::<T>(),
            allocated: AtomicUsize::new(0),
            #[cfg(feature = "persistence")]
            holes: 0,
            data: NonNull::from(Box::leak(data)).cast::<()>(),
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
            slot.0 < len && !self.is_hole(slot.0),
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
        // SAFETY: We exclusively own this page and its matching vtable.
        unsafe { (self.slot_vtable.drop_impl)(self) };
    }
}

/// A placeholder type representing the slots of an uninitialized `Page`.
struct DummySlot;

// SAFETY: The `DummySlot type is private.
unsafe impl Slot for DummySlot {
    unsafe fn memos(_: *const Self, _: Revision) -> *const MemoTable {
        unreachable!()
    }

    unsafe fn memos_unchecked(_: *const Self) -> *const MemoTable {
        unreachable!()
    }

    fn memos_mut(&mut self) -> &mut MemoTable {
        unreachable!()
    }
}

fn make_id(page: PageIndex, slot: SlotIndex) -> Id {
    let page = page.0 as u32;
    let slot = slot.0 as u32;
    // SAFETY: `slot` is guaranteed to be small enough that the resulting Id won't be bigger than `Id::MAX_U32`
    unsafe { Id::from_index((page << PAGE_LEN_BITS) | slot) }
}

#[inline]
pub fn split_id(id: Id) -> (PageIndex, SlotIndex) {
    let index = id.index() as usize;
    let slot = index & PAGE_LEN_MASK;
    let page = index >> PAGE_LEN_BITS;
    (PageIndex::new(page), SlotIndex::new(slot))
}
