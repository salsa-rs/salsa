#![cfg(all(feature = "persistence", feature = "inventory"))]

use salsa::{Database, Setter, Storage};
use std::sync::atomic::{AtomicUsize, Ordering};

#[salsa::db]
#[derive(Clone, Default)]
struct Db {
    storage: Storage<Self>,
}

#[salsa::db]
impl Database for Db {}

#[salsa::input(persist)]
struct Source {
    value: u32,
}

#[derive(PartialEq, Eq)]
struct Ast(u32);

fn fingerprint(ast: &Ast) -> u128 {
    u128::from(ast.0) << 96
}

static PARSES: AtomicUsize = AtomicUsize::new(0);
static READS: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked(persist(fingerprint = fingerprint))]
fn parse(db: &dyn Database, source: Source) -> Ast {
    PARSES.fetch_add(1, Ordering::SeqCst);
    Ast(source.value(db) % 2)
}

#[salsa::tracked(returns(copy))]
fn ephemeral_parse_value(db: &dyn Database, source: Source) -> u32 {
    parse(db, source).0
}

#[salsa::tracked(persist, returns(copy))]
fn public_type(db: &dyn Database, source: Source) -> u32 {
    READS.fetch_add(1, Ordering::SeqCst);
    ephemeral_parse_value(db, source)
}

#[salsa::tracked(returns(copy))]
fn check(db: &dyn Database, source: Source) -> u32 {
    public_type(db, source)
}

fn save(db: &mut Db, roots: &[salsa::DatabaseKeyIndex]) -> Vec<u8> {
    let mut bytes = Vec::new();
    <dyn Database>::write_cache(db, &mut bytes, 12345, roots).unwrap();
    bytes
}

thread_local! {
    static INPUT_DECODING_CONTEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct ContextualValue(u32);

impl<'de> serde::Deserialize<'de> for ContextualValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if !INPUT_DECODING_CONTEXT.get() {
            return Err(serde::de::Error::custom("missing input decoding context"));
        }
        u32::deserialize(d).map(Self)
    }
}

#[salsa::input(persist)]
struct ContextualInput {
    value: ContextualValue,
}

#[test]
fn input_fields_decode_on_the_thread_opening_the_cache() {
    let mut db = Db::default();
    let input = ContextualInput::new(&db, ContextualValue(7));
    let root = salsa::DatabaseKeyIndex::for_struct(&db, input);
    let image = save(&mut db, &[root]);
    INPUT_DECODING_CONTEXT.set(true);
    let loaded = restore(&image);
    INPUT_DECODING_CONTEXT.set(false);
    assert_eq!(input.value(&loaded).0, 7);
}

fn restore(bytes: &[u8]) -> Db {
    Db {
        storage: Storage::builder()
            .build_from_cache(bytes.to_vec(), 12345)
            .unwrap(),
    }
}

#[test]
fn retains_reachable_high_durability_results() {
    use salsa::Durability;

    static FACTS: AtomicUsize = AtomicUsize::new(0);
    static CLIENTS: AtomicUsize = AtomicUsize::new(0);

    #[salsa::tracked(persist, returns(copy))]
    fn fact(db: &dyn Database, source: Source) -> u32 {
        FACTS.fetch_add(1, Ordering::SeqCst);
        source.value(db) % 2
    }

    #[salsa::tracked(persist, returns(copy))]
    fn client(db: &dyn Database, source: Source) -> u32 {
        CLIENTS.fetch_add(1, Ordering::SeqCst);
        fact(db, source)
    }

    let mut db = Db::default();
    let medium = Source::builder(2)
        .value_durability(Durability::MEDIUM)
        .new(&db);
    let high = Source::builder(2)
        .value_durability(Durability::HIGH)
        .new(&db);
    let roots = [medium, high].map(|source| {
        assert_eq!(client(&db, source), 0);
        client::cache_key(&db, source)
    });
    let bytes = save(&mut db, &roots);
    let db = restore(&bytes);
    assert_eq!(fact(&db, medium), 0);
    assert_eq!(FACTS.load(Ordering::SeqCst), 2);
    assert_eq!(fact(&db, high), 0);
    assert_eq!(FACTS.load(Ordering::SeqCst), 2);

    let mut db = restore(&bytes);
    medium
        .set_value(&mut db)
        .with_durability(Durability::MEDIUM)
        .to(4);
    assert_eq!(client(&db, medium), 0);
    assert_eq!(CLIENTS.load(Ordering::SeqCst), 2);
    high.set_value(&mut db)
        .with_durability(Durability::HIGH)
        .to(5);
    assert_eq!(client(&db, high), 1);
    assert_eq!(CLIENTS.load(Ordering::SeqCst), 3);
    let bytes = save(&mut db, &roots);
    assert_eq!(client(&restore(&bytes), high), 1);
}

#[salsa::interned(persist)]
struct Symbol<'db> {
    #[returns(ref)]
    name: String,
}

#[salsa::tracked(persist, returns(ref))]
fn page_value(db: &dyn Database, source: Source) -> (Symbol<'_>, String) {
    let value = source.value(db);
    // The interned handle occurs only in this memo's dependency edges.
    Symbol::new(db, format!("dependency-{value:04}"));
    (
        Symbol::new(db, format!("object-{value:04}")),
        format!("result-{value:04}"),
    )
}

#[test]
fn replacement_copies_untouched_pages_and_collects_partially_live_pages() {
    use salsa::plumbing::{FromId, SalsaStructInDb, ZalsaDatabase};
    use std::io::{self, Write};

    struct ObserveCopies<'a> {
        original: &'a [u8],
        output: Vec<u8>,
        copied_query: bool,
        copied_object: bool,
    }
    impl Write for ObserveCopies<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.original.as_ptr_range().contains(&bytes.as_ptr()) {
                self.copied_query |= bytes.windows(11).any(|w| w == b"result-0000");
                self.copied_object |= bytes.windows(11).any(|w| w == b"object-0000");
            }
            self.output.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut db = Db::default();
    let sources: Vec<_> = (0..384).map(|n| Source::new(&db, n)).collect();
    let mut roots = Vec::new();
    for &source in &sources {
        page_value(&db, source);
        roots.push(page_value::cache_key(&db, source));
    }
    let original = bytes::Bytes::from(save(&mut db, &roots));
    let mut db = Db {
        storage: Storage::builder()
            .build_from_cache(original.clone(), 12345)
            .unwrap(),
    };
    roots.remove(160);
    sources[320].set_value(&mut db).to(1000);
    page_value(&db, sources[320]);
    let mut writer = ObserveCopies {
        original: &original,
        output: Vec::new(),
        copied_query: false,
        copied_object: false,
    };
    <dyn Database>::write_cache(&mut db, &mut writer, 12345, &roots).unwrap();
    assert!(writer.copied_query && writer.copied_object);
    let db = restore(&writer.output);
    assert_eq!(Source::entries(db.zalsa()).count(), 383);
    let symbols: std::collections::HashSet<_> = Symbol::entries(db.zalsa())
        .map(|key| Symbol::from_id(key.key_index()).name(&db).as_str())
        .collect();
    assert_eq!(symbols.len(), 2 * 383);
    assert!(symbols.contains("dependency-0000")); // forwarded page
    assert!(symbols.contains("dependency-0161")); // partially reachable page
    assert!(symbols.contains("dependency-1000")); // re-executed memo
    assert!(!symbols.contains("dependency-0160")); // omitted root
    assert!(!symbols.contains("dependency-0320")); // replaced memo
    for (n, source) in sources.into_iter().enumerate().filter(|(n, _)| *n != 160) {
        let expected = if n == 320 { 1000 } else { n };
        let (symbol, result) = page_value(&db, source);
        assert_eq!(symbol.name(&db), &format!("object-{expected:04}"));
        assert_eq!(result, &format!("result-{expected:04}"));
    }
}

#[salsa::tracked(persist)]
struct Node<'db> {
    #[returns(copy)]
    symbol: Symbol<'db>,
}

#[test]
fn durable_query_keeps_its_tracked_outputs() {
    static CREATED: AtomicUsize = AtomicUsize::new(0);

    #[salsa::tracked(specify, persist, returns(copy))]
    fn assigned(_db: &dyn Database, _node: Node<'_>) -> u32 {
        panic!("the producer supplies this value")
    }

    #[salsa::tracked(persist, returns(copy))]
    fn make(db: &dyn Database, source: Source) -> Node<'_> {
        CREATED.fetch_add(1, Ordering::SeqCst);
        let value = *source.value(db);
        let node = Node::new(db, Symbol::new(db, value.to_string()));
        assigned::specify(db, node, value);
        node
    }

    let mut db = Db::default();
    let source = Source::builder(7)
        .value_durability(salsa::Durability::HIGH)
        .new(&db);
    assert_eq!(make(&db, source).symbol(&db).name(&db), "7");
    let keys = [make::cache_key(&db, source)];
    let db = restore(&save(&mut db, &keys));
    assert_eq!(make(&db, source).symbol(&db).name(&db), "7");
    assert_eq!(assigned(&db, make(&db, source)), 7);
    assert_eq!(CREATED.load(Ordering::SeqCst), 1);
}

#[test]
fn reports_a_reachable_unsaved_assignment() {
    #[salsa::tracked(specify, returns(copy))]
    fn unsaved(_db: &dyn Database, _node: Node<'_>) -> u32 {
        0
    }

    #[salsa::tracked(persist, returns(copy))]
    fn owner(db: &dyn Database, source: Source) -> Node<'_> {
        let value = *source.value(db);
        let node = Node::new(db, Symbol::new(db, value.to_string()));
        unsaved::specify(db, node, value);
        node
    }

    let mut db = Db::default();
    let source = Source::new(&db, 7);
    assert_eq!(unsaved(&db, owner(&db, source)), 7);
    let roots = [owner::cache_key(&db, source)];
    let err = <dyn Database>::write_cache(&mut db, Vec::new(), 12345, &roots).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("unsaved"));
}

#[salsa::tracked]
fn create_node(db: &dyn Database, source: Source) -> Node<'_> {
    Node::new(db, Symbol::new(db, source.value(db).to_string()))
}

#[salsa::tracked(persist)]
fn exported_node(db: &dyn Database, source: Source) -> Node<'_> {
    *create_node(db, source)
}

#[test]
fn retains_the_ids_of_a_recomputed_producer() {
    use salsa::plumbing::AsId;
    let mut db = Db::default();
    let source = Source::new(&db, 12);
    let expected = exported_node(&db, source).as_id();
    let root = exported_node::cache_key(&db, source);
    let mut db = restore(&save(&mut db, &[root]));
    assert_eq!(exported_node(&db, source).as_id(), expected);
    assert_eq!(create_node(&db, source).as_id(), expected);
    let db = restore(&save(&mut db, &[root]));
    assert_eq!(create_node(&db, source).as_id(), expected);
}

#[salsa::input(persist)]
struct Handle {
    link: salsa::Id,
}

#[test]
fn collect_unreachable_objects_and_keep_ids_embedded_in_fields() {
    use salsa::plumbing::{AsId, FromId, SalsaStructInDb, ZalsaDatabase};
    let mut db = Db::default();
    let sources = [1, 2, 3].map(|v| Source::new(&db, v));
    let handles = sources.map(|source| Handle::new(&db, create_node(&db, source).as_id()));
    let root = salsa::DatabaseKeyIndex::for_struct(&db, handles[1]);
    let mut db = restore(&save(&mut db, &[root]));
    assert_eq!(Source::entries(db.zalsa()).count(), 0);
    assert_eq!(Handle::entries(db.zalsa()).count(), 1);
    assert_eq!(Node::entries(db.zalsa()).count(), 1);
    assert_eq!(Symbol::entries(db.zalsa()).count(), 1);
    let id = *handles[1].link(&db);
    let node = <Node<'_> as FromId>::from_id(id);
    assert_eq!(node.symbol(&db).name(&db), "2");
    let new_source = Source::new(&db, 4);
    let new = Handle::new(&db, create_node(&db, new_source).as_id());
    let other = salsa::DatabaseKeyIndex::for_struct(&db, new);
    let db = restore(&save(&mut db, &[other, root]));
    assert_eq!(Handle::entries(db.zalsa()).count(), 2);
    assert_eq!(Symbol::entries(db.zalsa()).count(), 2);
    assert_eq!(
        <Node<'_> as FromId>::from_id(*new.link(&db))
            .symbol(&db)
            .name(&db),
        "4"
    );
}

#[salsa::tracked]
struct Scratch<'db> {
    value: u32,
}

#[salsa::tracked]
fn produce_pair(db: &dyn Database, source: Source) -> (Node<'_>, Node<'_>) {
    let value = *source.value(db);
    Scratch::new(db, value);
    (
        Node::new(db, Symbol::new(db, value.to_string())),
        Node::new(db, Symbol::new(db, "unused".to_owned())),
    )
}

#[salsa::tracked(persist, returns(copy))]
fn first_node(db: &dyn Database, source: Source) -> Node<'_> {
    produce_pair(db, source).0
}

#[test]
fn identity_metadata_keeps_only_referenced_outputs() {
    use salsa::plumbing::{AsId, SalsaStructInDb, ZalsaDatabase};
    let mut db = Db::default();
    let source = Source::new(&db, 8);
    let id = first_node(&db, source).as_id();
    let root = first_node::cache_key(&db, source);
    let db = restore(&save(&mut db, &[root]));
    assert_eq!(Node::entries(db.zalsa()).count(), 1);
    // Interning both reusable symbols records two validation dependencies.
    assert_eq!(Symbol::entries(db.zalsa()).count(), 2);
    assert_eq!(produce_pair(&db, source).0.as_id(), id);
}

#[salsa::input(singleton)]
struct LocalSetting {
    value: u32,
}

#[salsa::tracked(persist, returns(copy))]
fn from_local_setting(db: &dyn Database, _source: Source) -> u32 {
    *LocalSetting::get(db).value(db)
}

#[test]
fn returns_an_error_for_an_unstored_input() {
    let mut db = Db::default();
    LocalSetting::new(&db, 1);
    let source = Source::new(&db, 2);
    assert_eq!(from_local_setting(&db, source), 1);
    let root = from_local_setting::cache_key(&db, source);
    assert!(<dyn Database>::write_cache(&mut db, Vec::new(), 1, &[root]).is_err());
}

#[cfg(feature = "accumulator")]
#[salsa::accumulator]
struct Notice(u32);

#[cfg(feature = "accumulator")]
#[salsa::tracked(persist, returns(copy))]
fn notices(db: &dyn Database, source: Source) -> u32 {
    use salsa::Accumulator;
    let number = *source.value(db);
    Notice(number).accumulate(db);
    number
}

#[cfg(feature = "accumulator")]
#[test]
fn cannot_silently_drop_accumulated_values() {
    let mut db = Db::default();
    let source = Source::new(&db, 7);
    assert_eq!(notices::accumulated::<Notice>(&db, source)[0].0, 7);
    let root = notices::cache_key(&db, source);
    assert_eq!(
        <dyn Database>::write_cache(&mut db, Vec::new(), 1, &[root])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
}

#[test]
fn restored_interning_reuses_the_same_ids_on_competing_threads() {
    use salsa::plumbing::AsId;
    let mut db = Db::default();
    let (roots, ids): (Vec<_>, Vec<_>) = (0..64)
        .map(|i| {
            let symbol = Symbol::new(&db, format!("symbol {i}"));
            (
                salsa::DatabaseKeyIndex::for_struct(&db, symbol),
                symbol.as_id(),
            )
        })
        .unzip();
    let db = restore(&save(&mut db, &roots));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let db = db.clone();
            let ids = &ids;
            scope.spawn(move || {
                for (i, id) in ids.iter().enumerate() {
                    assert_eq!(Symbol::new(&db, format!("symbol {i}")).as_id(), *id);
                }
            });
        }
    });
}

// Force all test values into one interner shard so the released slot is recycled.
#[derive(Clone, Eq, PartialEq, salsa::SalsaValue, serde::Serialize, serde::Deserialize)]
struct Colliding(u32);

impl std::hash::Hash for Colliding {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u8(0);
    }
}

#[salsa::interned(persist, revisions = 1)]
struct Recycled<'db> {
    value: Colliding,
}

#[salsa::tracked(persist, returns(copy))]
fn recycle(db: &dyn Database, source: Source) -> Recycled<'_> {
    Recycled::new(db, Colliding(*source.value(db)))
}

#[test]
fn reopened_slots_preserve_and_advance_the_interned_generation() {
    use salsa::plumbing::AsId;
    let mut db = Db::default();
    let source = Source::new(&db, 0);
    let first = recycle(&db, source).as_id();
    source.set_value(&mut db).to(1);
    let second = recycle(&db, source).as_id();
    assert_eq!(Some(second), first.next_generation());
    let root = recycle::cache_key(&db, source);
    let mut db = restore(&save(&mut db, &[root]));
    assert_eq!(recycle(&db, source).as_id(), second);
    assert_eq!(recycle(&db, source).value(&db).0, 1);
    source.set_value(&mut db).to(2);
    assert_eq!(Some(recycle(&db, source).as_id()), second.next_generation());
}

#[salsa::interned(persist, revisions = usize::MAX)]
struct Temporary<'db> {
    value: u32,
}

fn hash_temporary(value: &Temporary<'_>) -> u128 {
    serde_json::to_vec(value)
        .unwrap()
        .iter()
        .fold(0u128, |hash, byte| {
            hash.wrapping_mul(257).wrapping_add(u128::from(*byte))
        })
}

#[salsa::tracked(persist(fingerprint = hash_temporary), returns(copy))]
fn digest_only(db: &dyn Database, source: Source) -> Temporary<'_> {
    Temporary::new(db, *source.value(db))
}

#[test]
fn hashing_handles_does_not_persist_their_objects() {
    use salsa::plumbing::{SalsaStructInDb, ZalsaDatabase};
    let mut db = Db::default();
    let source = Source::new(&db, 6);
    assert_eq!(*digest_only(&db, source).value(&db), 6);
    let root = digest_only::cache_key(&db, source);
    let db = restore(&save(&mut db, &[root]));
    assert_eq!(Temporary::entries(db.zalsa()).count(), 0);
    assert_eq!(*digest_only(&db, source).value(&db), 6);
}

static DECODES: AtomicUsize = AtomicUsize::new(0);

#[derive(Eq, PartialEq, serde::Serialize)]
struct Deferred(u32);

impl<'de> serde::Deserialize<'de> for Deferred {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        DECODES.fetch_add(1, Ordering::SeqCst);
        u32::deserialize(d).map(Deferred)
    }
}

#[salsa::tracked(persist)]
fn deferred(db: &dyn Database, source: Source) -> Deferred {
    Deferred(*source.value(db))
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
struct RecomputeOnLoad(u32);

impl<'de> serde::Deserialize<'de> for RecomputeOnLoad {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom("unavailable cached value"))
    }
}

static FALLBACK_EXECUTIONS: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked(persist, returns(clone))]
fn fallible_value(db: &dyn Database, source: Source) -> RecomputeOnLoad {
    FALLBACK_EXECUTIONS.fetch_add(1, Ordering::SeqCst);
    RecomputeOnLoad(*source.value(db))
}

#[test]
fn reexecutes_after_a_lazy_value_cannot_be_decoded() {
    let mut db = Db::default();
    let source = Source::new(&db, 42);
    assert_eq!(fallible_value(&db, source), RecomputeOnLoad(42));
    let root = fallible_value::cache_key(&db, source);
    let mut db = restore(&save(&mut db, &[root]));
    assert_eq!(FALLBACK_EXECUTIONS.load(Ordering::SeqCst), 1);
    assert_eq!(fallible_value(&db, source), RecomputeOnLoad(42));
    assert_eq!(fallible_value(&db, source), RecomputeOnLoad(42));
    assert_eq!(FALLBACK_EXECUTIONS.load(Ordering::SeqCst), 2);
    assert!(<dyn Database>::reusable_cache(&mut db, 12345, &[root]).is_none());
}

#[test]
fn materialize_only_the_requested_value_once_across_threads() {
    let mut db = Db::default();
    let first = Source::new(&db, 1);
    let second = Source::new(&db, 2);
    assert_eq!(deferred(&db, first).0, 1);
    assert_eq!(deferred(&db, second).0, 2);
    let roots = [
        deferred::cache_key(&db, first),
        deferred::cache_key(&db, second),
    ];
    let original = save(&mut db, &roots);
    let mut db = restore(&original);
    let replay = save(&mut db, &[roots[1], roots[0]]);
    assert_eq!(original, replay);
    let db = restore(&replay);
    assert!(deferred::get_if_cached(&db, first).is_none());
    assert_eq!(DECODES.load(Ordering::SeqCst), 0);
    std::thread::scope(|s| {
        for _ in 0..4 {
            let db = db.clone();
            s.spawn(move || assert_eq!(deferred(&db, first).0, 1));
        }
    });
    assert_eq!(deferred::get_if_cached(&db, first).unwrap().0, 1);
    assert!(deferred::get_if_cached(&db, second).is_none());
    assert_eq!(DECODES.load(Ordering::SeqCst), 1);
}

static EXTERNAL: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked(returns(copy))]
fn read_external(db: &dyn Database, _source: Source) -> usize {
    db.report_untracked_read();
    EXTERNAL.load(Ordering::SeqCst)
}

#[salsa::tracked(persist, returns(copy))]
fn external_result(db: &dyn Database, source: Source) -> usize {
    read_external(db, source)
}

#[test]
fn restart_revalidates_transitive_untracked_reads() {
    let mut db = Db::default();
    let source = Source::new(&db, 0);
    assert_eq!(external_result(&db, source), 0);
    let root = external_result::cache_key(&db, source);
    let saved = save(&mut db, &[root]);
    EXTERNAL.store(9, Ordering::SeqCst);
    assert_eq!(external_result(&restore(&saved), source), 9);
}

#[test]
fn reuse_requires_the_same_roots_key_and_revision() {
    let mut db = Db::default();
    let source = Source::new(&db, 3);
    let key = salsa::DatabaseKeyIndex::for_struct(&db, source);
    let saved = save(&mut db, &[key]);
    let mut db = restore(&saved);
    let image = <dyn Database>::reusable_cache(&mut db, 12345, &[key, key]).unwrap();
    assert_eq!(image.as_ref(), saved.as_slice());
    assert!(<dyn Database>::reusable_cache(&mut db, 12345, &[]).is_none());
    assert!(<dyn Database>::reusable_cache(&mut db, 7, &[key]).is_none());
    source.set_value(&mut db).to(4);
    assert!(<dyn Database>::reusable_cache(&mut db, 12345, &[key]).is_none());
}

#[test]
fn only_reachable_query_instances_are_restored() {
    let mut db = Db::default();
    let live = Source::new(&db, 1);
    let dead = Source::new(&db, 2);
    assert_eq!(check(&db, live), 1);
    assert_eq!(check(&db, dead), 0);
    let root = check::cache_key(&db, live);
    let dead_input = salsa::DatabaseKeyIndex::for_struct(&db, dead);
    let saved = save(&mut db, &[root, root, dead_input]);
    assert!(
        Storage::<Db>::builder()
            .build_from_cache(saved.clone(), 0)
            .is_err()
    );
    assert!(
        Storage::<salsa::DatabaseImpl>::builder()
            .build_from_cache(saved.clone(), 12345)
            .is_err()
    );
    let mut damaged = saved.clone();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 1;
    assert_eq!(
        Storage::<Db>::builder()
            .build_from_cache(damaged, 12345)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(
        Storage::<Db>::builder()
            .build_from_cache(saved[..saved.len() - 1].to_vec(), 12345)
            .is_err()
    );

    let mut db = restore(&saved);
    // No invocation of the omitted root has run in this database yet.
    assert_eq!(save(&mut db, &[dead_input, root]), saved);
    assert_eq!(check(&db, live), 1);
    assert_eq!(READS.load(Ordering::SeqCst), 2);
    assert_eq!(PARSES.load(Ordering::SeqCst), 2);
    let saved = save(&mut db, &[root, dead_input]);
    let mut db = restore(&saved);
    live.set_value(&mut db).to(3);
    assert_eq!(check(&db, live), 1);
    assert_eq!(PARSES.load(Ordering::SeqCst), 3);
    assert_eq!(READS.load(Ordering::SeqCst), 2);
    assert_eq!(check(&db, dead), 0);
    assert_eq!(READS.load(Ordering::SeqCst), 3);
    live.set_value(&mut db).to(4);
    assert_eq!(check(&db, live), 0);
    assert_eq!(READS.load(Ordering::SeqCst), 4);
    let live_input = salsa::DatabaseKeyIndex::for_struct(&db, live);
    let db = restore(&save(&mut db, &[live_input]));
    assert_eq!(check(&db, live), 0);
    assert_eq!(READS.load(Ordering::SeqCst), 5);
}

#[test]
fn replacement_keeps_only_inputs_referenced_by_surviving_cached_origins() {
    use salsa::plumbing::{AsId, FromId, SalsaStructInDb, ZalsaDatabase};

    #[salsa::input(persist)]
    struct Reference {
        bits: u64,
    }

    #[salsa::tracked(persist, returns(copy))]
    fn read_reference(db: &dyn Database, input: Reference) -> u32 {
        // The ordinary scalar field and query result contain no serialized Salsa ID.
        // The source is reachable solely through the tracked field read in this origin.
        let source = Source::from_id(salsa::Id::from_bits(*input.bits(db)));
        *source.value(db)
    }

    let mut db = Db::default();
    let sources: Vec<_> = (0..8).map(|n| Source::new(&db, n)).collect();
    let references: Vec<_> = sources
        .iter()
        .map(|source| Reference::new(&db, source.as_id().as_bits()))
        .collect();
    let roots: Vec<_> = references
        .iter()
        .enumerate()
        .map(|(n, &input)| {
            assert_eq!(read_reference(&db, input), n as u32);
            read_reference::cache_key(&db, input)
        })
        .collect();
    let mut db = restore(&save(&mut db, &roots));
    assert_eq!(Source::entries(db.zalsa()).count(), 8);

    // Rewrite only part of the memo page. Five surviving rows still have
    // their opaque encoded values, and the sixth row has just re-executed.
    sources[0].set_value(&mut db).to(99);
    assert_eq!(read_reference(&db, references[0]), 99);
    let mut db = restore(&save(&mut db, &roots[..6]));

    assert_eq!(Reference::entries(db.zalsa()).count(), 6);
    assert_eq!(Source::entries(db.zalsa()).count(), 6);
    for n in 1..6 {
        assert_eq!(*sources[n].value(&db), n as u32);
        assert_eq!(read_reference(&db, references[n]), n as u32);
    }
    sources[1].set_value(&mut db).to(100);
    assert_eq!(read_reference(&db, references[1]), 100);
}
