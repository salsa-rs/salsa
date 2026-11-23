//! Same-binary snapshots of a reachable query graph.

mod page;

use std::any::TypeId;
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};

use rayon::prelude::*;
use serde::Serialize;
use serde::de::DeserializeSeed;

use crate::hash::FxHashSet;
use crate::zalsa::{ErasedJar, IngredientIndex, Zalsa};
use crate::zalsa_local::QueryOriginRef;
use crate::{Database, DatabaseKeyIndex, Id};
use rustc_hash::FxHashMap;

thread_local! {
    static REFERENCES: RefCell<Option<Vec<Id>>> = const { RefCell::new(None) };
}

pub(crate) fn trace_id(id: Id) {
    REFERENCES.with_borrow_mut(|references| {
        if let Some(references) = references {
            references.push(id);
        }
    });
}

/// A stored memo already describes its input and output keys in its origin. Include
/// their struct IDs in the enclosing page's reachability, without also storing an
/// ID list for every copy of the same edge in an opaque value record.
pub(crate) fn trace_origin(origin: QueryOriginRef<'_>) {
    REFERENCES.with_borrow_mut(|references| {
        if let Some(references) = references {
            match origin {
                QueryOriginRef::Assigned(key) => references.push(key.key_index()),
                QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => {
                    references.extend(edges.into_iter().map(|edge| edge.key().key_index()));
                }
            }
        }
    });
}

struct ReferenceScope(Option<Vec<Id>>);

impl Drop for ReferenceScope {
    fn drop(&mut self) {
        REFERENCES.replace(self.0.take());
    }
}

/// A fingerprint may use serde internally, but its value is not part of the snapshot.
pub(crate) fn without_references<T>(f: impl FnOnce() -> T) -> T {
    let _scope = ReferenceScope(REFERENCES.take());
    f()
}

/// Collect IDs visited by ordinary serde implementations, including IDs embedded
/// in user values that never appeared as query dependencies.
pub(crate) fn with_references<T>(f: impl FnOnce() -> T) -> (T, Vec<Id>) {
    let _scope = ReferenceScope(REFERENCES.replace(Some(Vec::new())));
    let value = f();
    (value, REFERENCES.take().unwrap())
}

/// Keep the object IDs next to opaque memo records so another snapshot can trace
/// the record without decoding its value. Nearby slots usually fit in one delta byte.
pub(crate) fn encode_references(ids: &mut Vec<Id>) -> io::Result<bytes::Bytes> {
    ids.sort_unstable_by_key(|id| id.as_bits());
    ids.dedup();
    let mut previous = 0;
    let mut encoded = Vec::with_capacity(ids.len());
    REFERENCES.with_borrow_mut(|outer| {
        if let Some(outer) = outer {
            outer.extend_from_slice(ids);
        }
    });
    for id in ids {
        let bits = id.as_bits();
        bincode::encode_into_std_write(bits - previous, &mut encoded, bincode::config::standard())
            .map_err(invalid)?;
        previous = bits;
    }
    Ok(encoded.into())
}

pub(crate) fn trace_saved_references(bytes: &[u8]) -> io::Result<()> {
    REFERENCES.with_borrow_mut(|references| {
        visit_references(bytes, |id| {
            if let Some(references) = references {
                references.push(id);
            }
        })
    })
}

fn visit_references(mut bytes: &[u8], mut visit: impl FnMut(Id)) -> io::Result<()> {
    let mut previous = 0u64;
    while !bytes.is_empty() {
        let (delta, used): (u64, _) =
            bincode::decode_from_slice(bytes, bincode::config::standard()).map_err(invalid)?;
        let bits = previous
            .checked_add(delta)
            .filter(|bits| *bits as u32 != 0)
            .ok_or_else(|| invalid("invalid object ID in cached memo"))?;
        visit(Id::from_bits(bits));
        previous = bits;
        bytes = &bytes[used..];
    }
    Ok(())
}

const MAGIC: &[u8; 8] = b"SALSADB5";

#[derive(Default)]
pub struct IdentityRows(
    pub(crate) FxHashMap<DatabaseKeyIndex, Vec<(crate::tracked_struct::Identity, Id)>>,
);

pub(crate) struct CacheImage {
    bytes: bytes::Bytes,
    pages: page::SavedPages,
    key: u128,
    roots: Vec<(u32, u64)>,
    ephemeral_roots: FxHashSet<(u32, u64)>,
    revision: crate::Revision,
    modified: std::sync::atomic::AtomicBool,
}

impl CacheImage {
    fn current<'a>(zalsa: &'a Zalsa, key: u128, roots: &[(u32, u64)]) -> Option<&'a Self> {
        let image = zalsa.cache_image.as_deref()?;
        (image.key == key
            && image.revision == zalsa.current_revision()
            && !image.modified.load(std::sync::atomic::Ordering::Relaxed)
            && image.roots == roots)
            .then_some(image)
    }

    pub(crate) fn memo_replaced(
        &self,
        key: DatabaseKeyIndex,
        persistent: bool,
        origin: QueryOriginRef<'_>,
    ) {
        use std::sync::atomic::Ordering::Relaxed;
        // Executing an omitted root can change which dependencies the next save must keep.
        // Other transient reads, such as formatting diagnostics, leave the snapshot intact.
        if !self.modified.load(Relaxed)
            && (persistent
                || origin.outputs().next().is_some()
                || (!self.ephemeral_roots.is_empty()
                    && self
                        .ephemeral_roots
                        .contains(&(key.ingredient_index().as_u32(), key.key_index().as_bits()))))
        {
            self.modified.store(true, Relaxed);
        }
    }
}

fn layout(zalsa: &Zalsa) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for ingredient in zalsa.ingredients() {
        ingredient.type_id().hash(&mut hash);
    }
    hash.finish()
}

fn type_tag(id: TypeId) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    hash.finish()
}

struct CheckedWriter<'a, W> {
    inner: &'a mut W,
    checksum: crc32fast::Hasher,
}

impl<W: Write> Write for CheckedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.checksum.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Default)]
struct SeenQueries {
    pages: FxHashMap<u64, u128>,
    recycled: FxHashSet<DatabaseKeyIndex>,
}

impl SeenQueries {
    fn insert(&mut self, key: DatabaseKeyIndex) -> bool {
        let id = key.key_index();
        if id.generation() != 0 {
            return self.recycled.insert(key);
        }
        let page =
            (u64::from(key.ingredient_index().as_u32()) << 32) | u64::from(id.index() / u128::BITS);
        let slots = self.pages.entry(page).or_default();
        let bit = 1u128 << (id.index() % u128::BITS);
        let fresh = *slots & bit == 0;
        *slots |= bit;
        fresh
    }
}

#[derive(Default)]
struct QueryBatch<'db> {
    children: Vec<DatabaseKeyIndex>,
    retained: Vec<(IngredientIndex, DatabaseKeyIndex)>,
    producers: Vec<(
        DatabaseKeyIndex,
        &'db [(crate::tracked_struct::Identity, Id)],
    )>,
    untracked: bool,
}

impl<'db> QueryBatch<'db> {
    fn trace(&mut self, zalsa: &'db Zalsa, keys: &[DatabaseKeyIndex]) -> io::Result<()> {
        for &key in keys {
            // SAFETY: The writer drained all other database handles before spawning workers.
            let Some((owner, memo)) = (unsafe { zalsa.cache_memo(key) }) else {
                self.untracked |= zalsa.is_cache_function(key.ingredient_index());
                continue;
            };
            #[cfg(feature = "accumulator")]
            if memo.has_accumulated_values() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "accumulated values cannot be persisted",
                ));
            }
            let ingredient = zalsa.lookup_ingredient(key.ingredient_index());
            if ingredient.is_persistable() {
                self.retained.push((
                    key.ingredient_index(),
                    DatabaseKeyIndex::new(owner, key.key_index()),
                ));
            } else {
                let identities = memo.cache_identities();
                if !identities.is_empty() {
                    self.producers.push((key, identities));
                }
            }
            let origin = memo.cache_origin();
            self.untracked |= matches!(origin, QueryOriginRef::DerivedUntracked(_));
            match origin {
                QueryOriginRef::Assigned(assigner) => {
                    if !ingredient.is_persistable() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "specified query `{}` must be persistable",
                                ingredient.debug_name()
                            ),
                        ));
                    }
                    self.children.push(assigner);
                }
                QueryOriginRef::Derived(edges) | QueryOriginRef::DerivedUntracked(edges) => {
                    self.children.extend(
                        edges
                            .into_iter()
                            .map(|edge| edge.key())
                            .filter(|key| zalsa.is_cache_function(key.ingredient_index())),
                    );
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    bincode::serde::encode_to_vec(value, bincode::config::standard()).map_err(invalid)
}

pub(crate) fn decode_seed<'de>(
    bytes: &'de [u8],
    seed: impl DeserializeSeed<'de, Value = ()>,
) -> io::Result<()> {
    let mut decoder =
        bincode::serde::BorrowedSerdeDecoder::from_slice(bytes, bincode::config::standard(), ());
    seed.deserialize(decoder.as_deserializer()).map_err(invalid)
}

fn root_ids(roots: &[DatabaseKeyIndex]) -> Vec<(u32, u64)> {
    let mut ids: Vec<_> = roots
        .iter()
        .map(|key| (key.ingredient_index().as_u32(), key.key_index().as_bits()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

impl dyn Database {
    /// Returns the original snapshot bytes if they still describe these roots and cache key.
    /// The bytes share the storage passed to `StorageBuilder::build_from_cache`.
    ///
    /// A caller that still has the original cache file can leave it in place. If the
    /// destination is different, these bytes can be written directly without re-encoding.
    /// A newly constructed database, an input change, or a different root set requires
    /// a call to `write_cache`.
    pub fn reusable_cache(
        &mut self,
        cache_key: u128,
        roots: &[DatabaseKeyIndex],
    ) -> Option<bytes::Bytes> {
        let zalsa = self.zalsa_mut();
        CacheImage::current(zalsa, cache_key, &root_ids(roots)).map(|image| image.bytes.clone())
    }

    /// Writes a binary snapshot for the given query invocations. Obtain each root
    /// with `query::cache_key(db, args...)` after computing the query.
    /// A root may itself be a non-persisted query.
    ///
    /// Only reachable persistable query results and fingerprints are included.
    /// Reads of an omitted query are replaced by its dependencies, down to the
    /// nearest retained query or input. Retaining an intermediate query lets its
    /// unchanged result or fingerprint stop invalidation after one of those inputs changes.
    /// Retains struct keys, dependencies, and Salsa IDs serialized by reachable values.
    /// Add externally held objects with [`DatabaseKeyIndex::for_struct`].
    /// Encoding runs in parallel while the database is exclusively borrowed.
    ///
    /// The application supplies a cache key identifying its exact build and data layout.
    /// Custom serializers must encode handles as Salsa IDs and be able to run on worker
    /// threads without a thread-local database attachment. When replacing a cache on disk,
    /// write to a temporary file and rename it after a successful write.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failures, non-persistable inputs reachable from a stored query,
    /// or accumulated values, which do not have a persistence representation. A reachable
    /// specified value and its assigning query must both support persistence.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "inventory", feature = "macros"))]
    /// # fn example() -> std::io::Result<()> {
    /// #[salsa::db]
    /// #[derive(Default, Clone)]
    /// struct Db { storage: salsa::Storage<Self> }
    /// #[salsa::db]
    /// impl salsa::Database for Db {}
    ///
    /// #[salsa::input(persist)]
    /// struct Source { number: u32 }
    /// #[salsa::tracked(persist, returns(copy))]
    /// fn answer(db: &dyn salsa::Database, source: Source) -> u32 {
    ///     source.number(db) * 2
    /// }
    ///
    /// let mut db = Db::default();
    /// let source = Source::new(&db, 21);
    /// assert_eq!(answer(&db, source), 42);
    /// let roots = [answer::cache_key(&db, source)];
    /// let mut bytes = Vec::new();
    /// let build_key = 123_u128;
    /// <dyn salsa::Database>::write_cache(&mut db, &mut bytes, build_key, &roots)?;
    /// let db = Db { storage: salsa::Storage::builder().build_from_cache(bytes, build_key)? };
    /// assert_eq!(answer(&db, source), 42);
    /// # Ok(()) }
    /// # #[cfg(all(feature = "inventory", feature = "macros"))]
    /// # example().unwrap();
    /// ```
    pub fn write_cache(
        &mut self,
        mut writer: impl Write,
        cache_key: u128,
        roots: &[DatabaseKeyIndex],
    ) -> io::Result<()> {
        let zalsa = &*self.zalsa_mut();
        let root_ids = root_ids(roots);
        if let Some(image) = CacheImage::current(zalsa, cache_key, &root_ids) {
            return writer.write_all(&image.bytes);
        }
        let mut pending = roots.to_vec();
        let mut seen = SeenQueries::default();
        let mut has_untracked_reads = false;
        let mut creators = FxHashMap::default();
        let mut keys = FxHashMap::<IngredientIndex, Vec<DatabaseKeyIndex>>::default();
        let mut sequential = QueryBatch::default();
        // Deduplicate a frontier before parallel workers inspect its memos. The shared
        // dependency graph requires no locks, even when many roots use the same queries.
        while !pending.is_empty() {
            pending.retain(|key| seen.insert(*key));
            const BATCH_SIZE: usize = 256;
            let mut parallel;
            let batches = if pending.len() <= BATCH_SIZE {
                // Long chains can have only one ready query at a time. Reuse the buffers
                // and avoid submitting work to the pool in this case.
                sequential.trace(zalsa, &pending)?;
                std::slice::from_mut(&mut sequential)
            } else {
                parallel = pending
                    .par_chunks(BATCH_SIZE)
                    .map(|chunk| {
                        let mut batch = QueryBatch::default();
                        batch.trace(zalsa, chunk)?;
                        Ok(batch)
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                parallel.as_mut_slice()
            };
            pending.clear();
            for batch in batches {
                pending.append(&mut batch.children);
                has_untracked_reads |= batch.untracked;
                for (ingredient, owner) in batch.retained.drain(..) {
                    keys.entry(ingredient).or_default().push(owner);
                }
                for (key, outputs) in batch.producers.drain(..) {
                    creators.extend(outputs.iter().map(|&(identity, id)| (id, (key, identity))));
                }
            }
        }

        // Encode the query values first so that serde also discovers Salsa IDs in their payloads.
        let previous = zalsa
            .cache_image
            .as_deref()
            .filter(|image| image.key == cache_key)
            .map(|image| &image.pages);
        let encode_batch = |batch: FxHashMap<IngredientIndex, Vec<DatabaseKeyIndex>>,
                            identities: &IdentityRows| {
            let mut groups = batch.into_iter().collect::<Vec<_>>();
            groups
                .par_iter_mut()
                .flat_map(|(index, selected)| {
                    let ingredient = zalsa.lookup_ingredient(*index);
                    let chunks: Vec<_> = if ingredient.supports_cache_pages() {
                        selected.sort_unstable_by_key(|key| key.key_index().index());
                        selected
                            .chunk_by(|a, b| page::slot_range(a) == page::slot_range(b))
                            .collect()
                    } else {
                        selected
                            .chunks(if ingredient.as_function().is_some() {
                                32
                            } else {
                                256
                            })
                            .collect()
                    };
                    chunks.into_par_iter().map(move |chunk| {
                        page::encode(zalsa, *index, ingredient, chunk, identities, previous)
                    })
                })
                .collect::<io::Result<Vec<_>>>()
        };
        let mut pending = Vec::new();
        for key in roots {
            if let Some(owner) = zalsa.table().allocated_ingredient(key.key_index())
                && (owner == key.ingredient_index()
                    || (zalsa.accepts_cache_key(key.ingredient_index(), owner)
                        && zalsa.lookup_ingredient(owner).is_persistable()))
            {
                pending.push(key.key_index());
            }
        }
        let mut query_blocks = Vec::new();
        let empty = IdentityRows::default();
        for (block, references) in encode_batch(keys, &empty)? {
            query_blocks.push(block);
            pending.extend(references);
        }

        let mut blocks = Vec::new();
        // One bit per allocated slot. Object tracing has always treated generations
        // of a slot as one object, while query keys retain their full generation.
        let mut marked = vec![0u128; zalsa.table().page_count()];
        let mut identities = IdentityRows::default();
        while !pending.is_empty() {
            let mut producer_keys = Vec::new();
            let mut batch = FxHashMap::<IngredientIndex, Vec<DatabaseKeyIndex>>::default();
            for id in pending.drain(..) {
                let slots = marked
                    .get_mut((id.index() / u128::BITS) as usize)
                    .ok_or_else(|| invalid("dangling Salsa ID in cache"))?;
                let bit = 1u128 << (id.index() % u128::BITS);
                if *slots & bit != 0 {
                    continue;
                }
                *slots |= bit;
                let owner = zalsa
                    .table()
                    .allocated_ingredient(id)
                    .ok_or_else(|| invalid("dangling Salsa ID in cache"))?;
                let ingredient = zalsa.lookup_ingredient(owner);
                if !ingredient.is_persistable() {
                    return Err(invalid(format!(
                        "reachable struct `{}` does not support persistence",
                        ingredient.debug_name()
                    )));
                }
                // The value of an omitted producer is never written. Save the identity
                // of each of its reachable outputs so that executing it can reuse that ID.
                if let Some(&(creator, identity)) =
                    creators.get(&id).or_else(|| zalsa.cache_creators.get(&id))
                {
                    identities
                        .0
                        .entry(creator)
                        .or_default()
                        .push((identity, id));
                    producer_keys.push(creator.key_index());
                }
                batch
                    .entry(owner)
                    .or_default()
                    .push(DatabaseKeyIndex::new(owner, id));
            }
            pending.extend(producer_keys);
            for (block, references) in encode_batch(batch, &empty)? {
                blocks.push(block);
                pending.extend(references);
            }
        }
        let mut producers = FxHashMap::<_, Vec<_>>::default();
        for key in identities.0.keys() {
            let owner = zalsa.table().ingredient_index(key.key_index());
            producers
                .entry(key.ingredient_index())
                .or_default()
                .push(DatabaseKeyIndex::new(owner, key.key_index()));
        }
        // Slots must be installed before the memos keyed by those slots.
        blocks.extend(query_blocks);
        blocks.extend(
            encode_batch(producers, &identities)?
                .into_iter()
                .map(|(block, _)| block),
        );
        let runtime = encode(zalsa.runtime())?;
        // Root keys must describe the same objects after reopening the snapshot.
        let replayable = !has_untracked_reads
            && roots.iter().all(|root| {
                zalsa
                    .table()
                    .allocated_ingredient(root.key_index())
                    .is_some_and(|owner| zalsa.lookup_ingredient(owner).is_persistable())
            });
        writer.write_all(MAGIC)?;
        writer.write_all(&cache_key.to_le_bytes())?;
        writer.write_all(&type_tag(zalsa.views().source_type_id()).to_le_bytes())?;
        let mut registrations: Vec<_> = zalsa
            .registered_jars()
            .map(|(id, name)| (type_tag(id), name))
            .collect();
        registrations.sort_unstable_by_key(|&(id, _)| id);
        let mut checked = CheckedWriter {
            inner: &mut writer,
            checksum: crc32fast::Hasher::new(),
        };
        let directory = page::directory(&blocks);
        let data = blocks
            .iter()
            .map(|block| (block.index, block.page.data.as_ref()))
            .collect::<Vec<_>>();
        bincode::encode_into_std_write(
            (
                (registrations, layout(zalsa)),
                replayable.then_some(root_ids),
                has_untracked_reads,
                runtime,
                data,
                directory,
            ),
            &mut checked,
            bincode::config::standard(),
        )
        .map_err(io::Error::other)?;
        let checksum = checked.checksum.finalize();
        writer.write_all(&checksum.to_le_bytes())
    }
}

/// Check the jar set before constructing any ingredients. In a manually registered
/// database, constructing a function jar already requires its argument struct's jar.
pub(crate) fn read_header<Db: Database>(
    bytes: &bytes::Bytes,
    cache_key: u128,
    manual: &[ErasedJar],
) -> io::Result<(u64, bytes::Bytes)> {
    let body = bytes
        .strip_prefix(MAGIC)
        .ok_or_else(|| invalid("unsupported Salsa cache format"))?;
    let body = body
        .strip_prefix(&cache_key.to_le_bytes())
        .ok_or_else(|| invalid("Salsa cache key differs"))?;
    let body = body
        .strip_prefix(&type_tag(TypeId::of::<Db>()).to_le_bytes())
        .ok_or_else(|| invalid("Salsa cache belongs to a different database type"))?;
    let (body, checksum) = body
        .split_last_chunk::<4>()
        .ok_or_else(|| invalid("truncated Salsa cache"))?;
    if crc32fast::hash(body).to_le_bytes() != *checksum {
        return Err(invalid("incomplete or corrupt Salsa cache"));
    }
    type Registry<'a> = (Vec<(u64, &'a str)>, u64);
    let ((required, layout), used): (Registry<'_>, _) =
        bincode::borrow_decode_from_slice(body, bincode::config::standard()).map_err(invalid)?;
    let mut available: FxHashMap<_, _> = ErasedJar::all(manual.iter().copied())
        .map(|jar| (type_tag(jar.jar_type_id()), jar))
        .collect();
    for (id, name) in required {
        if available.remove(&id).is_none() {
            return Err(invalid(format!(
                "Salsa cache requires unregistered ingredient `{name}`"
            )));
        }
    }
    if let Some(jar) = available.values().next() {
        return Err(invalid(format!(
            "database ingredient `{}` is absent from the Salsa cache",
            jar.type_name()
        )));
    }
    Ok((layout, bytes.slice_ref(&body[used..])))
}

pub(crate) fn restore(
    zalsa: &mut Zalsa,
    bytes: bytes::Bytes,
    cache_key: u128,
    expected_layout: u64,
    body: &[u8],
) -> io::Result<()> {
    if layout(zalsa) != expected_layout {
        return Err(invalid("Salsa cache ingredient layout differs"));
    }
    type Image<'a> = (
        Option<Vec<(u32, u64)>>,
        bool,
        &'a [u8],
        Vec<(u32, &'a [u8])>,
        page::Directory<'a>,
    );
    let ((roots, has_untracked_reads, runtime, blocks, directory), used): (Image<'_>, _) =
        bincode::borrow_decode_from_slice(body, bincode::config::standard()).map_err(invalid)?;
    if used != body.len() {
        return Err(invalid("trailing data in Salsa cache"));
    }
    let (mut runtime, _) =
        bincode::serde::decode_from_slice(runtime, bincode::config::standard()).map_err(invalid)?;
    zalsa.runtime_mut().deserialize_from(&mut runtime);
    // Install the object slots first. Once their addresses are stable, the memo tables
    // permit parallel insertion without removing query ingredients from the registry.
    let count = zalsa.ingredients().count();
    let pages = page::restore(&bytes, &blocks, directory)?;
    let mut object_blocks = vec![Vec::new(); count];
    let mut memo_blocks = Vec::new();
    for (raw_index, block) in blocks {
        if raw_index as usize >= count {
            return Err(invalid("unknown ingredient in Salsa cache"));
        }
        let index = IngredientIndex::new(raw_index);
        let ingredient = zalsa.lookup_ingredient(index);
        let block = bytes.slice_ref(block);
        if ingredient.as_function().is_some() {
            memo_blocks.push((index, block));
        } else if ingredient.is_persistable() {
            object_blocks[raw_index as usize].push(block);
        } else {
            return Err(invalid("unexpected ingredient in Salsa cache"));
        }
    }
    // Every table page belongs to exactly one ingredient. Give each worker exclusive
    // access to that ingredient and its pages, then publish the finished allocations.
    let (parallel, local): (Vec<_>, Vec<_>) = zalsa
        .ingredients_mut()
        .zip(object_blocks)
        .filter(|(_, blocks)| !blocks.is_empty())
        .partition(|(ingredient, _)| ingredient.parallel_cache_decode());
    let restore =
        |(ingredient, blocks): (&mut dyn crate::ingredient::Ingredient, Vec<bytes::Bytes>)| {
            let mut pages = crate::table::RestoredPages::default();
            for block in blocks {
                ingredient.decode_cache(&mut pages, &block)?;
            }
            Ok::<_, io::Error>(pages)
        };
    let mut worker_pages = Ok(Vec::new());
    let local_pages = rayon::in_place_scope(|scope| {
        scope.spawn(|_| worker_pages = parallel.into_par_iter().map(restore).collect());
        local
            .into_iter()
            .map(restore)
            .collect::<io::Result<Vec<_>>>()
    });
    for pages in local_pages?.into_iter().chain(worker_pages?) {
        zalsa
            .table_mut()
            .install_cache_pages(pages)
            .map_err(invalid)?;
    }
    let creators = memo_blocks
        .par_iter()
        .map(|(index, block)| {
            zalsa
                .lookup_ingredient(*index)
                .decode_cache_memos(zalsa, block)
        })
        .collect::<io::Result<Vec<_>>>()?;
    for group in creators {
        for (producer, outputs) in group.0 {
            for (identity, id) in outputs {
                zalsa.cache_creators.insert(id, (producer, identity));
            }
        }
    }
    if has_untracked_reads {
        zalsa.new_revision();
    } else if let Some(roots) = roots {
        let ephemeral_roots = roots
            .iter()
            .copied()
            .filter(|&(index, _)| {
                let index = IngredientIndex::new(index);
                zalsa.is_cache_function(index) && !zalsa.lookup_ingredient(index).is_persistable()
            })
            .collect();
        zalsa.cache_image = Some(Box::new(CacheImage {
            ephemeral_roots,
            bytes,
            pages,
            key: cache_key,
            roots,
            revision: zalsa.current_revision(),
            modified: std::sync::atomic::AtomicBool::new(false),
        }));
    }
    Ok(())
}
