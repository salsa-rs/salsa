//! Storage for a key's erased memo pointers. Only this module can access the
//! encoded word. The borrowed table views supply the registered array length.

use std::mem;
use std::ptr::{self, NonNull};

use super::{DummyMemo, MemoEntry, MemoTableWithTypes, MemoTableWithTypesMut};
use crate::sync::atomic::{AtomicPtr, Ordering};

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
    pub(super) fn load(&self, index: usize) -> Option<NonNull<DummyMemo>> {
        let word = self.memos.entries.word.load(Ordering::Acquire);

        match decode(word, self.types.len()) {
            Entries::Empty => None,
            Entries::Inline(first, memo) => (index == first).then_some(memo),
            Entries::Array(array) => {
                // SAFETY: Acquire observes the published array. Its length is
                // fixed by the type list and it stays allocated under shared access.
                let entries = unsafe { &*array };
                NonNull::new(entries.get(index)?.atomic_memo.load(Ordering::Acquire))
            }
        }
    }

    pub(super) fn store(
        &self,
        index: usize,
        memo: NonNull<DummyMemo>,
    ) -> Option<NonNull<DummyMemo>> {
        let word = &self.memos.entries.word;
        let mut state = word.load(Ordering::Acquire);

        loop {
            let inline = match decode(state, self.types.len()) {
                Entries::Empty => None,
                Entries::Inline(first, previous) => Some((first, previous)),
                Entries::Array(array) => {
                    // SAFETY: As in `load`, the published array stays in place.
                    let entries = unsafe { &*array };
                    return NonNull::new(
                        entries[index]
                            .atomic_memo
                            .swap(memo.as_ptr(), Ordering::AcqRel),
                    );
                }
            };

            if inline.is_none_or(|(first, _)| first == index)
                && let Some(new) = encode_inline(index, memo)
            {
                match word.compare_exchange(state, new, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return inline.map(|(_, old)| old),
                    Err(current) => state = current,
                }
            } else {
                match self.promote(state, inline, index, memo) {
                    Ok(old) => return NonNull::new(old),
                    Err(current) => state = current,
                }
            }
        }
    }

    #[cold]
    fn promote(
        &self,
        state: *mut MemoEntry,
        inline: Option<(usize, NonNull<DummyMemo>)>,
        index: usize,
        memo: NonNull<DummyMemo>,
    ) -> Result<*mut DummyMemo, *mut MemoEntry> {
        let len = self.types.len();
        // Collecting defaults can compile to a zeroed allocation, which is
        // more expensive for small memo tables with jemalloc.
        let mut entries = Vec::with_capacity(len);
        entries.resize_with(len, MemoEntry::default);

        if let Some((first, previous)) = inline {
            *entries[first].atomic_memo.get_mut() = previous.as_ptr();
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
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(old),
            Err(current) => {
                // SAFETY: This array was never published. Dropping its atomic
                // pointers frees the array without dropping either boxed memo.
                unsafe { drop(Box::from_raw(array)) };
                Err(current)
            }
        }
    }

    #[cfg(feature = "salsa_unstable")]
    pub(super) fn measure_entries<T>(
        &self,
        mut measure: impl FnMut(usize, NonNull<DummyMemo>) -> T,
    ) -> (usize, Vec<T>) {
        let word = self.memos.entries.word.load(Ordering::Acquire);

        match decode(word, self.types.len()) {
            Entries::Empty => (0, Vec::new()),
            Entries::Inline(index, memo) => (0, vec![measure(index, memo)]),
            Entries::Array(array) => {
                // SAFETY: As in `load`, the published array stays in place.
                let entries = unsafe { &*array };
                let memos = entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        let memo = NonNull::new(entry.atomic_memo.load(Ordering::Acquire))?;
                        Some(measure(index, memo))
                    })
                    .collect();

                (mem::size_of_val(entries), memos)
            }
        }
    }
}

impl MemoTableWithTypesMut<'_> {
    /// Detaches each pointer before handing it to the callback. If the callback
    /// panics, the caller's drop guard can drain the remaining pointers.
    pub(super) fn drain(&mut self, mut take: impl FnMut(usize, NonNull<DummyMemo>)) {
        let word = self.memos.entries.word.get_mut();

        match decode(*word, self.types.len()) {
            Entries::Empty => {}
            Entries::Inline(index, memo) => {
                *word = ptr::null_mut();
                take(index, memo);
            }
            Entries::Array(array) => {
                // SAFETY: We have exclusive access to the published array.
                let entries = unsafe { &mut *array };
                for (index, entry) in entries.iter_mut().enumerate() {
                    let memo = mem::replace(entry.atomic_memo.get_mut(), ptr::null_mut());
                    if let Some(memo) = NonNull::new(memo) {
                        take(index, memo);
                    }
                }

                *word = ptr::null_mut();
                // SAFETY: We detached the boxed slice with its original length.
                unsafe { drop(Box::from_raw(array)) };
            }
        }
    }
}

/// A decoded snapshot. Only the atomic word is stored in each key.
enum Entries {
    Empty,
    Inline(usize, NonNull<DummyMemo>),
    Array(*mut [MemoEntry]),
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
fn decode(word: *mut MemoEntry, len: usize) -> Entries {
    if word.is_null() {
        return Entries::Empty;
    }

    if word.addr() & 1 == 0 {
        return Entries::Array(ptr::slice_from_raw_parts_mut(word, len));
    }

    let index = (word.addr() >> 1) & MAX_INLINE_INDEX;
    let memo = word.map_addr(|addr| {
        if cfg!(target_pointer_width = "32") {
            addr & !TAG_MASK
        } else {
            addr >> TAG_BITS
        }
    });

    Entries::Inline(index, NonNull::new(memo.cast()).unwrap())
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
                assert!(matches!(decode(state, types.len()), Entries::Array(_)));
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
            let Entries::Inline(first, memo) = decode(word, index + 1) else {
                panic!("expected inline memo");
            };
            assert_eq!((first, memo), (index, pointer(max_address)));
        }
        assert!(encode_inline(MAX_INLINE_INDEX + 1, pointer(max_address)).is_none());
        assert!(encode_inline(0, pointer(invalid_address)).is_none());
    }
}
