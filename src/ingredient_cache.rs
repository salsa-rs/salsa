use std::marker::PhantomData;
use std::mem;
use std::num::NonZeroU32;

use crate::nonce::Nonce;
use crate::plumbing::Ingredient;
use crate::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::zalsa::{StorageNonce, Zalsa};
use crate::IngredientIndex;

/// Caches an ingredient index.
///
/// Unlike [`IngredientCache`], this is not restricted to a specific database.
/// Note that ingredients are statically registered with `inventory`, so their
/// indices should be stable across any databases.
///
/// If ingredient initialization is database dependent, e.g. for registering
/// view casters, [`IngredientCache`] should be used instead.
pub struct GlobalIngredientCache<I>
where
    I: Ingredient,
{
    ingredient_index: AtomicU32,
    phantom: PhantomData<fn() -> I>,
}

impl<I> Default for GlobalIngredientCache<I>
where
    I: Ingredient,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<I> GlobalIngredientCache<I>
where
    I: Ingredient,
{
    const UNINITIALIZED: u32 = u32::MAX;

    /// Create a new cache
    pub const fn new() -> Self {
        Self {
            ingredient_index: AtomicU32::new(Self::UNINITIALIZED),
            phantom: PhantomData,
        }
    }

    /// Get a reference to the ingredient in the database.
    ///
    /// If the ingredient index is not already in the cache, it will be loaded and cached.
    #[inline(always)]
    pub fn get_or_create<'db>(
        &self,
        zalsa: &'db Zalsa,
        load_index: impl Fn() -> IngredientIndex,
    ) -> &'db I {
        const _: () = assert!(
            mem::size_of::<(Nonce<StorageNonce>, IngredientIndex)>() == mem::size_of::<u64>()
        );

        let mut ingredient_index = self.ingredient_index.load(Ordering::Acquire);
        if ingredient_index == Self::UNINITIALIZED {
            ingredient_index = self.get_or_create_index_slow(load_index).as_u32();
        };

        zalsa
            .lookup_ingredient(IngredientIndex::from_unchecked(ingredient_index))
            .assert_type()
    }

    #[cold]
    #[inline(never)]
    fn get_or_create_index_slow(
        &self,
        load_index: impl Fn() -> IngredientIndex,
    ) -> IngredientIndex {
        let ingredient_index = load_index();

        // It doesn't matter if we overwrite any stores, as `create_index` should
        // always return the same index.
        self.ingredient_index
            .store(ingredient_index.as_u32(), Ordering::Release);

        ingredient_index
    }
}

/// Caches ingredient initialization in a specific database.
///
/// Optimized for the case of a single database.
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
    #[inline(always)]
    pub fn get_or_create<'db>(
        &self,
        zalsa: &'db Zalsa,
        create_index: impl Fn() -> (IngredientIndex, &'db I),
    ) -> &'db I {
        const _: () = assert!(
            mem::size_of::<(Nonce<StorageNonce>, IngredientIndex)>() == mem::size_of::<u64>()
        );

        let cached_data = self.cached_data.load(Ordering::Acquire);
        if cached_data == Self::UNINITIALIZED {
            return self.get_or_create_index_slow(zalsa, create_index);
        };

        // Unpack our `u64` into the nonce and index.
        let index = IngredientIndex::from_unchecked(cached_data as u32);

        // SAFETY: We've checked against `UNINITIALIZED` (0) above and so the upper bits must be non-zero.
        let nonce = Nonce::<StorageNonce>::from_u32(unsafe {
            NonZeroU32::new_unchecked((cached_data >> u32::BITS) as u32)
        });

        // The data was cached for a different database, we have to ensure the ingredient was
        // created in ours.
        if zalsa.nonce() != nonce {
            let (_, ingredient) = create_index();
            return ingredient;
        }

        zalsa.lookup_ingredient(index).assert_type()
    }

    #[cold]
    #[inline(never)]
    fn get_or_create_index_slow<'db>(
        &self,
        zalsa: &'db Zalsa,
        create_index: impl Fn() -> (IngredientIndex, &'db I),
    ) -> &'db I {
        let (index, ingredient) = create_index();
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
        ingredient
    }
}
