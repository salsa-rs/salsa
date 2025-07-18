pub use imp::IngredientCache;

#[cfg(feature = "inventory")]
mod imp {
    use crate::plumbing::Ingredient;
    use crate::sync::atomic::{self, AtomicU32, Ordering};
    use crate::zalsa::Zalsa;
    use crate::IngredientIndex;

    use std::marker::PhantomData;

    /// Caches an ingredient index.
    ///
    /// Note that all ingredients are statically registered with `inventory`, so their
    /// indices should be stable across any databases.
    pub struct IngredientCache<I>
    where
        I: Ingredient,
    {
        ingredient_index: AtomicU32,
        phantom: PhantomData<fn() -> I>,
    }

    impl<I> Default for IngredientCache<I>
    where
        I: Ingredient,
    {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<I> IngredientCache<I>
    where
        I: Ingredient,
    {
        const UNINITIALIZED: u32 = u32::MAX;

        /// Create a new cache
        pub const fn new() -> Self {
            Self {
                ingredient_index: atomic::AtomicU32::new(Self::UNINITIALIZED),
                phantom: PhantomData,
            }
        }

        /// Get a reference to the ingredient in the database.
        ///
        /// If the ingredient index is not already in the cache, it will be loaded and cached.
        ///
        /// # Safety
        ///
        /// The `IngredientIndex` returned by the closure must reference a valid ingredient of
        /// type `I` in the provided zalsa database.
        pub unsafe fn get_or_create<'db>(
            &self,
            zalsa: &'db Zalsa,
            load_index: impl Fn() -> IngredientIndex,
        ) -> &'db I {
            let mut ingredient_index = self.ingredient_index.load(Ordering::Acquire);
            if ingredient_index == Self::UNINITIALIZED {
                ingredient_index = self.get_or_create_index_slow(load_index).as_u32();
            };

            // SAFETY: `ingredient_index` is initialized from a valid `IngredientIndex`.
            let ingredient_index = unsafe { IngredientIndex::new_unchecked(ingredient_index) };

            // SAFETY: There are a two cases here:
            // - The `create_index` closure was called due to the data being uncached. In this
            //   case, the caller guarantees the index is in-bounds and has the correct type.
            // - The index was cached. While the current database might not be the same database
            //   the ingredient was initially loaded from, the `inventory` feature is enabled, so
            //   ingredient indices are stable across databases. Thus the index is still in-bounds
            //   and has the correct type.
            unsafe {
                zalsa
                    .lookup_ingredient_unchecked(ingredient_index)
                    .assert_type_unchecked()
            }
        }

        #[cold]
        #[inline(never)]
        fn get_or_create_index_slow(
            &self,
            load_index: impl Fn() -> IngredientIndex,
        ) -> IngredientIndex {
            let ingredient_index = load_index();

            // It doesn't matter if we overwrite any stores, as `create_index` should
            // always return the same index when the `inventory` feature is enabled.
            self.ingredient_index
                .store(ingredient_index.as_u32(), Ordering::Release);

            ingredient_index
        }
    }
}

#[cfg(not(feature = "inventory"))]
mod imp {
    use crate::nonce::Nonce;
    use crate::plumbing::Ingredient;
    use crate::sync::atomic::{AtomicU64, Ordering};
    use crate::zalsa::{StorageNonce, Zalsa};
    use crate::IngredientIndex;

    use std::marker::PhantomData;
    use std::mem;

    /// Caches an ingredient index.
    ///
    /// With manual registration, ingredient indices can vary across databases,
    /// but we can retain most of the benefit by optimizing for the the case of
    /// a single database.
    pub struct IngredientCache<I>
    where
        I: Ingredient,
    {
        // A packed representation of `Option<(Nonce<StorageNonce>, IngredientIndex)>`.
        //
        // This allows us to replace a lock in favor of an atomic load. This works thanks to `Nonce`
        // having a niche, which means the entire type can fit into an `AtomicU64`.
        cached_data: AtomicU64,
        phantom: PhantomData<fn() -> I>,
    }

    impl<I> Default for IngredientCache<I>
    where
        I: Ingredient,
    {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<I> IngredientCache<I>
    where
        I: Ingredient,
    {
        const UNINITIALIZED: u64 = 0;

        /// Create a new cache
        pub const fn new() -> Self {
            Self {
                cached_data: AtomicU64::new(Self::UNINITIALIZED),
                phantom: PhantomData,
            }
        }

        /// Get a reference to the ingredient in the database.
        ///
        /// If the ingredient is not already in the cache, it will be created.
        ///
        /// # Safety
        ///
        /// The `IngredientIndex` returned by the closure must reference a valid ingredient of
        /// type `I` in the provided zalsa database.
        #[inline(always)]
        pub unsafe fn get_or_create<'db>(
            &self,
            zalsa: &'db Zalsa,
            create_index: impl Fn() -> IngredientIndex,
        ) -> &'db I {
            let index = self.get_or_create_index(zalsa, create_index);

            // SAFETY: There are a two cases here:
            // - The `create_index` closure was called due to the data being uncached for the
            //   provided database. In this case, the caller guarantees the index is in-bounds
            //   and has the correct type.
            // - We verified the index was cached for the same database, by the nonce check.
            //   Thus the initial safety argument still applies.
            unsafe {
                zalsa
                    .lookup_ingredient_unchecked(index)
                    .assert_type_unchecked::<I>()
            }
        }

        pub fn get_or_create_index(
            &self,
            zalsa: &Zalsa,
            create_index: impl Fn() -> IngredientIndex,
        ) -> IngredientIndex {
            const _: () = assert!(
                mem::size_of::<(Nonce<StorageNonce>, IngredientIndex)>() == mem::size_of::<u64>()
            );

            let cached_data = self.cached_data.load(Ordering::Acquire);
            if cached_data == Self::UNINITIALIZED {
                return self.get_or_create_index_slow(zalsa, create_index);
            };

            // Unpack our `u64` into the nonce and index.
            //
            // SAFETY: The lower bits of `cached_data` are initialized from a valid `IngredientIndex`.
            let index = unsafe { IngredientIndex::new_unchecked(cached_data as u32) };

            // SAFETY: We've checked against `UNINITIALIZED` (0) above and so the upper bits must be non-zero.
            let nonce = crate::nonce::Nonce::<StorageNonce>::from_u32(unsafe {
                std::num::NonZeroU32::new_unchecked((cached_data >> u32::BITS) as u32)
            });

            // The data was cached for a different database, we have to ensure the ingredient was
            // created in ours.
            if zalsa.nonce() != nonce {
                return create_index();
            }

            index
        }

        #[cold]
        #[inline(never)]
        fn get_or_create_index_slow(
            &self,
            zalsa: &Zalsa,
            create_index: impl Fn() -> IngredientIndex,
        ) -> IngredientIndex {
            let index = create_index();
            let nonce = zalsa.nonce().into_u32().get() as u64;
            let packed = (nonce << u32::BITS) | (index.as_u32() as u64);
            debug_assert_ne!(packed, IngredientCache::<I>::UNINITIALIZED);

            // Discard the result, whether we won over the cache or not doesn't matter.
            _ = self.cached_data.compare_exchange(
                IngredientCache::<I>::UNINITIALIZED,
                packed,
                Ordering::Release,
                Ordering::Relaxed,
            );

            // Use our locally computed index regardless of which one was cached.
            index
        }
    }
}
