//! Storage for a key's erased memo pointers. Only this module can access the
//! encoded word. The borrowed table views supply the registered array length.

use std::mem;
use std::ptr::{self, NonNull};

use super::{DummyMemo, Memo, MemoEntry, MemoTableWithTypes, MemoTableWithTypesMut};
use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::zalsa::MemoIngredientIndex;

#[derive(Default)]
pub(super) struct MemoEntries {
    /// Null means empty. A non-null word with bit 0 clear points to an array
    /// with one entry per registered query. Array pointers are at least two-byte aligned.
    ///
    /// With bit 0 set, the word holds one boxed memo's address and query index.
    /// The most significant bits are on the left.
    ///
    /// ```text
    /// 64-bit  [ address bits 55..0 ][ index bits 6..0 ][ 1 ]
    /// 32-bit  [ address bits 31..4 ][ index bits 2..0 ][ 1 ]
    /// ```
    ///
    /// On 64-bit targets the memo address must have a zero top byte and the
    /// index must be at most 127. On 32-bit targets the memo address must be
    /// 16-byte aligned and the index at most 7. Otherwise we use the array.
    ///
    /// For example, index 3 and address `0x1230` encode as `0x123007` on
    /// 64-bit and `0x1237` on 32-bit.
    word: AtomicPtr<MemoEntry>,
}

impl MemoTableWithTypes<'_> {
    #[inline]
    pub(super) fn entries(&self) -> Entries<'_> {
        self.snapshot().1
    }

    /// Keep the observed word for a writer's compare-exchange.
    #[inline]
    fn snapshot(&self) -> (*mut MemoEntry, Entries<'_>) {
        let word = self.memos.entries.word.load(Ordering::Acquire);
        let entries = if word.is_null() {
            Entries::Empty
        } else if let Some((index, memo)) = decode_inline(word) {
            Entries::Inline(index, memo)
        } else {
            // SAFETY: Acquire observes the initialized array. Its length is
            // fixed by the type list and it stays allocated under shared access.
            Entries::Array(unsafe { std::slice::from_raw_parts(word, self.types.len()) })
        };

        (word, entries)
    }

    pub(super) fn store(&self, index: usize, memo: NonNull<DummyMemo>) -> *mut DummyMemo {
        let word = &self.memos.entries.word;

        // After a failed compare-exchange the next snapshot acquires the
        // winning writer's word before we access its entries.
        loop {
            let (state, entries) = self.snapshot();
            let inline = match entries {
                Entries::Empty => None,
                Entries::Inline(first, previous) => Some((first, previous)),
                Entries::Array(entries) => {
                    return entries[index]
                        .atomic_memo
                        .swap(memo.as_ptr(), Ordering::AcqRel);
                }
            };

            if inline.is_none_or(|(first, _)| first == index)
                && let Some(new) = encode_inline(index, memo)
            {
                if word
                    .compare_exchange(state, new, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return inline.map_or(ptr::null_mut(), |(_, old)| old);
                }
            } else if let Ok(old) = self.promote(state, inline, index, memo) {
                return old;
            }
        }
    }

    #[cold]
    fn promote(
        &self,
        state: *mut MemoEntry,
        inline: Option<(usize, *mut DummyMemo)>,
        index: usize,
        memo: NonNull<DummyMemo>,
    ) -> Result<*mut DummyMemo, ()> {
        let len = self.types.len();
        // Collecting defaults can compile to a zeroed allocation, which is
        // more expensive for small memo tables with jemalloc.
        let mut entries = Vec::with_capacity(len);
        entries.resize_with(len, MemoEntry::default);

        if let Some((first, previous)) = inline {
            *entries[first].atomic_memo.get_mut() = previous;
        }

        // A replacement at the same index can also require promotion if its
        // address no longer fits. Return the old pointer to the caller.
        let old = mem::replace(entries[index].atomic_memo.get_mut(), memo.as_ptr());
        let array = Box::into_raw(entries.into_boxed_slice());

        // Publish both entries together. If another writer replaced the inline
        // memo or published an array, retry with its state. Replaced memos stay
        // alive until the end of the revision, so their addresses can't be reused.
        match self.memos.entries.word.compare_exchange(
            state,
            array.cast(),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(old),
            Err(_) => {
                // SAFETY: This array was never published. Dropping its atomic
                // pointers frees the array without dropping either boxed memo.
                unsafe { drop(Box::from_raw(array)) };
                Err(())
            }
        }
    }
}

/// A borrowed view of the occupied entries. Only the atomic word is stored in the key.
pub(super) enum Entries<'a> {
    Empty,
    Inline(usize, *mut DummyMemo),
    Array(&'a [MemoEntry]),
}

impl Entries<'_> {
    #[inline]
    pub(super) fn get(&self, index: usize) -> Option<NonNull<DummyMemo>> {
        match *self {
            Self::Inline(first, memo) if index == first => NonNull::new(memo),
            Self::Empty | Self::Inline(..) => None,
            Self::Array(entries) => {
                NonNull::new(entries.get(index)?.atomic_memo.load(Ordering::Acquire))
            }
        }
    }

    #[cfg(feature = "salsa_unstable")]
    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, NonNull<DummyMemo>)> {
        let (inline, array) = match *self {
            Self::Empty => (None, &[][..]),
            Self::Inline(index, memo) => (Some((index, NonNull::new(memo).unwrap())), &[][..]),
            Self::Array(entries) => (None, entries),
        };

        inline
            .into_iter()
            .chain(array.iter().enumerate().filter_map(|(index, entry)| {
                let memo = NonNull::new(entry.atomic_memo.load(Ordering::Acquire))?;
                Some((index, memo))
            }))
    }

    #[cfg(feature = "salsa_unstable")]
    pub(super) fn heap_size(&self) -> usize {
        match self {
            Self::Empty | Self::Inline(..) => 0,
            Self::Array(entries) => mem::size_of_val(*entries),
        }
    }
}

impl MemoTableWithTypesMut<'_> {
    /// Removes the memos in index order and transfers ownership of each to `f`.
    ///
    /// # Safety
    ///
    /// There must be no outstanding references to the memos.
    pub(crate) unsafe fn take_memos(
        &mut self,
        mut f: impl FnMut(MemoIngredientIndex, Box<dyn Memo>),
    ) {
        let word = self.memos.entries.word.get_mut();
        if word.is_null() {
            return;
        }

        if let Some((index, memo)) = decode_inline(*word) {
            *word = ptr::null_mut();
            let type_ = &self.types.types[index];
            let memo = NonNull::new(memo).unwrap();
            // SAFETY: We detached the original box. Insertion checked its type
            // and the caller guarantees that no reader still borrows it.
            let memo = unsafe { Box::from_raw((type_.to_dyn_fn)(memo).as_ptr()) };
            f(MemoIngredientIndex::from_usize(index), memo);
            return;
        }

        let array = ptr::slice_from_raw_parts_mut(*word, self.types.len());
        // SAFETY: We have exclusive access to the published array. The type
        // list gives its original length and the concrete type at each index.
        let entries = unsafe { &mut *array };
        for (index, (entry, type_)) in entries.iter_mut().zip(&self.types.types).enumerate() {
            // SAFETY: The types match and the caller guarantees no outstanding borrows.
            if let Some(memo) = unsafe { entry.take(type_) } {
                f(MemoIngredientIndex::from_usize(index), memo);
            }
        }

        // Keep the allocation attached until the last callback returns so the
        // caller's drop guard can clean up the remaining memos if one panics.
        *word = ptr::null_mut();
        // SAFETY: We detached the boxed slice with its original length.
        unsafe { drop(Box::from_raw(array)) };
    }
}

// On 64-bit targets, checked multiplication by 256 makes room for the tag
// only if the top byte is zero. Shifting right by 8 restores the address.
// On 32-bit targets, that would limit us to addresses below 16 MiB. Instead,
// we check that the memo is 16-byte aligned so we can use its four low bits
// for the tag and clear them to recover the address.
//
// If the address or index doesn't fit, we allocate an array. The tagged
// pointer may point outside the memo allocation, but `with_addr` and
// `map_addr` preserve its provenance. We only dereference the pointer
// after restoring the original address.
const TAG_BITS: u32 = if cfg!(target_pointer_width = "32") {
    4
} else {
    8
};
const TAG_MASK: usize = (1 << TAG_BITS) - 1;
const MAX_INLINE_INDEX: usize = (1 << (TAG_BITS - 1)) - 1;

#[inline]
fn encode_inline(index: usize, memo: NonNull<DummyMemo>) -> Option<*mut MemoEntry> {
    const { assert!(mem::align_of::<MemoEntry>() >= 2) };
    let ptr = memo.as_ptr();
    if index > MAX_INLINE_INDEX {
        return None;
    }

    let tag = (index << 1) | 1;
    let address = if cfg!(target_pointer_width = "32") {
        if ptr.addr() & TAG_MASK != 0 {
            return None;
        }
        ptr.addr()
    } else {
        ptr.addr().checked_mul(1 << TAG_BITS)?
    };
    Some(ptr.with_addr(address | tag).cast())
}

#[inline]
fn decode_inline(word: *mut MemoEntry) -> Option<(usize, *mut DummyMemo)> {
    if word.addr() & 1 == 0 {
        return None;
    }

    let index = (word.addr() >> 1) & MAX_INLINE_INDEX;
    let memo = word.map_addr(|addr| {
        if cfg!(target_pointer_width = "32") {
            addr & !TAG_MASK
        } else {
            addr >> TAG_BITS
        }
    });

    Some((index, memo.cast()))
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::super::{Memo, MemoEntryType, MemoTable, MemoTableTypes};
    use super::*;
    use crate::DatabaseKeyIndex;
    use crate::zalsa::{MemoIngredientIndex, Zalsa};

    #[cfg_attr(target_pointer_width = "32", repr(align(16)))]
    struct TestMemo(usize);

    impl Memo for TestMemo {
        fn has_value(&self) -> bool {
            true
        }

        fn remove_outputs(&self, _: &Zalsa, _: DatabaseKeyIndex) {}

        #[cfg(feature = "salsa_unstable")]
        fn memory_usage(&self) -> crate::database::MemoInfo {
            unreachable!()
        }
    }

    fn types(len: usize) -> MemoTableTypes {
        MemoTableTypes {
            types: (0..len).map(|_| MemoEntryType::of::<TestMemo>()).collect(),
        }
    }

    fn memo(value: usize) -> NonNull<TestMemo> {
        NonNull::new(Box::into_raw(Box::new(TestMemo(value)))).unwrap()
    }

    #[test]
    fn promotion_preserves_memo_addresses_and_iteration_order() {
        let types = types(129);
        let mut memos = MemoTable::default();
        let index = MemoIngredientIndex::from_usize;
        // Exercise both an inline first insertion and a first index too large
        // to encode. Reuse the same table after draining it.
        for first in [MAX_INLINE_INDEX, MAX_INLINE_INDEX + 1] {
            // SAFETY: This test uses the same type list for every access to `memos`.
            let table = || unsafe { types.attach_memos(&memos) };
            let original = memo(1);
            assert!(table().insert(index(first), original).is_none());
            let state = memos.entries.word.load(Ordering::Acquire);
            if let Some(encoded) = encode_inline(first, original.cast()) {
                assert_eq!(state, encoded);
            } else {
                assert!(matches!(table().entries(), Entries::Array(_)));
            }
            assert!(table().get::<TestMemo>(index(0)).is_none());
            assert!(table().insert(index(0), memo(2)).is_none());
            assert_eq!(table().get(index(first)), Some(original));
            let mut order = Vec::new();
            // SAFETY: This table has no outstanding readers.
            unsafe {
                types
                    .attach_memos_mut(&mut memos)
                    .take_memos(|i, _| order.push(i.as_usize()))
            };
            assert_eq!(order, [0, first]);
            assert!(memos.entries.word.get_mut().is_null());
        }
    }

    #[test]
    fn concurrent_replacement_and_promotion_keep_all_memos() {
        let types = types(3);
        let mut memos = MemoTable::default();
        let index = MemoIngredientIndex::from_usize;
        // SAFETY: All threads share the same immutable type list for `memos`.
        let table = || unsafe { types.attach_memos(&memos) };
        let first = memo(0);
        assert!(table().insert(index(1), first).is_none());
        let start = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            for i in [0, 2] {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    assert!(table().insert(index(i), memo(i)).is_none());
                });
            }
            start.wait();
            assert_eq!(table().insert(index(1), memo(1)), Some(first));
        });
        for i in 0..3 {
            let memo = table().get::<TestMemo>(index(i)).unwrap();
            // SAFETY: The table owns these allocations and all writers have joined.
            assert_eq!(unsafe { memo.as_ref() }.0, i);
        }
        // SAFETY: Replacement returned ownership of the first allocation.
        // All readers and writers have finished using the table.
        unsafe {
            drop(Box::from_raw(first.as_ptr()));
            types.attach_memos_mut(&mut memos).drop();
        }
    }

    #[test]
    fn inline_encoding_is_lossless_or_falls_back() {
        let pointer = |address| NonNull::new(ptr::without_provenance_mut(address)).unwrap();
        let (max_address, invalid_address) = if cfg!(target_pointer_width = "32") {
            (usize::MAX & !TAG_MASK, 4)
        } else {
            let max_address = usize::MAX >> TAG_BITS;
            (max_address, max_address + 1)
        };
        for index in 0..=MAX_INLINE_INDEX {
            let word = encode_inline(index, pointer(max_address)).unwrap();
            assert_eq!(
                decode_inline(word),
                Some((index, pointer(max_address).as_ptr()))
            );
        }
        assert!(encode_inline(MAX_INLINE_INDEX + 1, pointer(max_address)).is_none());
        assert!(encode_inline(0, pointer(invalid_address)).is_none());
    }
}
