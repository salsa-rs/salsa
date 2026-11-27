use std::ops::{Index, Range};
use std::ptr;

use super::{MAX_PAGES, Page};
use crate::sync::Mutex;
use crate::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// Buckets hold 32, 64, 128, ... headers, starting at indices 0, 32, 96, ...
const FIRST_BUCKET_LEN: usize = 32;
const BUCKET_COUNT: usize = bucket_index(MAX_PAGES - 1) + 1;

#[inline]
const fn bucket_index(index: usize) -> usize {
    // Adding the first bucket length makes each bucket start at a power of two.
    // Page indices come from the high bits of a u32 slot index.
    ((index as u32 + FIRST_BUCKET_LEN as u32).ilog2() - FIRST_BUCKET_LEN.ilog2()) as usize
}

const fn bucket_start(bucket: usize) -> usize {
    (FIRST_BUCKET_LEN << bucket) - FIRST_BUCKET_LEN
}

fn bucket_capacity(bucket: usize) -> usize {
    (FIRST_BUCKET_LEN << bucket).min(MAX_PAGES - bucket_start(bucket))
}

// A 64-byte stride lets readers scale the page index with a single shift.
#[cfg_attr(target_pointer_width = "64", repr(align(64)))]
struct Header(Page);

/// An append-only directory of stable page headers.
///
/// A worker appends once per new page, but each slot lookup reads the directory.
/// Serializing appends lets readers validate the bucket and header with one
/// bounds check. Every index below `len` has an initialized header.
pub(super) struct Pages {
    /// Each pointer is offset backwards by the bucket's starting index.
    /// For example, the second bucket starts at page 32, so adding 35 to
    /// its adjusted base addresses the fourth header in the allocation.
    bases: [AtomicPtr<Header>; BUCKET_COUNT],
    len: AtomicUsize,
    append: Mutex<()>,
}

impl Pages {
    pub(super) fn new() -> Self {
        Self {
            bases: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            len: AtomicUsize::new(0),
            append: Mutex::new(()),
        }
    }

    pub(super) fn push(&self, page: Page) -> usize {
        let _guard = self.append.lock();
        let index = self.len.load(Ordering::Relaxed);
        assert!(index < MAX_PAGES, "Salsa page limit exceeded");
        let bucket = bucket_index(index);
        if index == bucket_start(bucket) {
            let allocation = Box::<[Header]>::new_uninit_slice(bucket_capacity(bucket));
            let first = Box::into_raw(allocation).cast::<Header>();
            // The adjusted base may be outside the allocation, even null.
            // Only use it with wrapping_add to return to the same allocation.
            self.bases[bucket].store(first.wrapping_sub(index), Ordering::Relaxed);
        }
        let header = self.bases[bucket]
            .load(Ordering::Relaxed)
            .wrapping_add(index);
        // SAFETY: The append lock gives us exclusive access to this uninitialized
        // header. Earlier headers stay at their original addresses.
        unsafe { header.write(Header(page)) };
        self.len.store(index + 1, Ordering::Release);
        index
    }

    #[inline]
    fn header(&self, index: usize) -> Option<*mut Header> {
        if index >= self.count() {
            return None;
        }
        // The acquire on len observes the publishing writer's release. The
        // append lock orders all earlier writers before that release too.
        let base = self.bases[bucket_index(index)].load(Ordering::Relaxed);
        Some(base.wrapping_add(index))
    }

    #[inline]
    fn get(&self, index: usize) -> Option<&Page> {
        let header = self.header(index)?;
        // SAFETY: header checked publication. Headers are stable and remain
        // live until the directory is exclusively borrowed or dropped.
        Some(unsafe { &(*header).0 })
    }

    pub(super) fn get_mut(&mut self, index: usize) -> Option<&mut Page> {
        let header = self.header(index)?;
        // SAFETY: This directory owns the initialized header and is exclusively borrowed.
        Some(unsafe { &mut (*header).0 })
    }

    #[inline]
    pub(super) fn count(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub(super) fn iter(&self) -> Iter<'_> {
        Iter {
            pages: self,
            indices: 0..self.count(),
        }
    }
}

impl Index<usize> for Pages {
    type Output = Page;

    #[inline]
    fn index(&self, index: usize) -> &Page {
        #[cold]
        #[inline(never)]
        fn assert_failed(index: usize) -> ! {
            panic!("index {index} is uninitialized");
        }

        match self.get(index) {
            Some(page) => page,
            None => assert_failed(index),
        }
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        let len = *self.len.get_mut();
        for (bucket, base) in self.bases.iter_mut().enumerate() {
            let start = bucket_start(bucket);
            if start >= len {
                break;
            }
            let capacity = bucket_capacity(bucket);
            let initialized = capacity.min(len - start);
            let first = base.get_mut().wrapping_add(start);
            // SAFETY: Adding the start index recovers the original allocation
            // pointer. The boxed slice of MaybeUninit<Header> has the same
            // allocation layout as a Vec<Header> with this capacity. Only
            // the initialized prefix is live, and we exclusively own it.
            unsafe { drop(Vec::from_raw_parts(first, initialized, capacity)) };
        }
    }
}

pub(super) struct Iter<'a> {
    pages: &'a Pages,
    indices: Range<usize>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (usize, &'a Page);

    fn next(&mut self) -> Option<Self::Item> {
        self.indices.next().map(|i| (i, &self.pages[i]))
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;
    use crate::IngredientIndex;
    use crate::table::{DummySlot, MemoTableTypes};
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn concurrent_appends_across_buckets() {
        const WRITERS: usize = 4;
        const PER_WRITER: usize = bucket_start(2) / WRITERS;

        let types = Arc::new(MemoTableTypes::default());
        let mut pages = Pages::new();
        let push = |i| {
            pages.push(Page::new::<DummySlot>(
                IngredientIndex::new(i),
                types.clone(),
            ))
        };
        assert_eq!(push(0), 0);
        let first = &pages[0];
        assert!(pages.bases[1].load(Ordering::Relaxed).is_null());
        let barrier = Barrier::new(WRITERS);
        thread::scope(|scope| {
            for i in 1..=WRITERS as u32 {
                let (barrier, push, pages) = (&barrier, &push, &pages);
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..PER_WRITER {
                        let idx = push(i);
                        assert_eq!(pages[idx].ingredient, IngredientIndex::new(i));
                        assert!(pages.get(idx - 1).is_some());
                    }
                });
            }
        });
        assert!(std::ptr::eq(first, &pages[0]));
        assert_eq!(pages.count(), bucket_start(2) + 1);
        assert!(pages.iter().map(|(i, _)| i).eq(0..pages.count()));
        assert!(pages.get(pages.count()).is_none());
        assert!(pages.get(usize::MAX).is_none());
        assert!(pages.bases[3].load(Ordering::Relaxed).is_null());
        pages.get_mut(bucket_start(1)).unwrap().ingredient = IngredientIndex::new(9);
        assert_eq!(pages[bucket_start(1)].ingredient, IngredientIndex::new(9));
        drop(pages);
        assert_eq!(Arc::strong_count(&types), 1);
    }

    #[test]
    fn bucket_boundaries_cover_the_id_range() {
        for i in 0..BUCKET_COUNT {
            let first = bucket_start(i);
            let end = first + bucket_capacity(i);
            assert_eq!(bucket_index(first), i);
            assert_eq!(bucket_index(end - 1), i);
            assert_eq!(
                end,
                if i + 1 == BUCKET_COUNT {
                    MAX_PAGES
                } else {
                    bucket_start(i + 1)
                }
            );
        }
    }
}
