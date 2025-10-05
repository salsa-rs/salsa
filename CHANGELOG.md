# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.24.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.23.0...salsa-v0.24.0) - 2025-09-30

### Fixed

- Cleanup provisional cycle head memos when query panics ([#993](https://github.com/salsa-rs/salsa/pull/993))
- Runaway for unchanged queries participating in cycle ([#981](https://github.com/salsa-rs/salsa/pull/981))
- Delete not re-created tracked structs after fixpoint iteration ([#979](https://github.com/salsa-rs/salsa/pull/979))
- fix assertion during interned deserialization ([#978](https://github.com/salsa-rs/salsa/pull/978))
- Do not unnecessarily require `Debug` on fields for interned structs ([#951](https://github.com/salsa-rs/salsa/pull/951))
- Fix phantom data usage in salsa structs affecting auto traits ([#932](https://github.com/salsa-rs/salsa/pull/932))

### Other

- Replace unsafe unwrap with `expect` call ([#998](https://github.com/salsa-rs/salsa/pull/998))
- Push active query in execute ([#996](https://github.com/salsa-rs/salsa/pull/996))
- Update codspeed action ([#997](https://github.com/salsa-rs/salsa/pull/997))
- Add implementations for Lookup and HashEqLike for CompactString ([#988](https://github.com/salsa-rs/salsa/pull/988))
- Provide a method to attach a database even if it's different from the current attached one ([#992](https://github.com/salsa-rs/salsa/pull/992))
- Allow fallback to take longer than one iteration to converge ([#991](https://github.com/salsa-rs/salsa/pull/991))
- refactor `entries` API ([#987](https://github.com/salsa-rs/salsa/pull/987))
- Persistent caching fixes ([#982](https://github.com/salsa-rs/salsa/pull/982))
- outline cold path of `lookup_ingredient` ([#984](https://github.com/salsa-rs/salsa/pull/984))
- Update snapshot to fix nightly type rendering ([#983](https://github.com/salsa-rs/salsa/pull/983))
- avoid cycles during serialization ([#977](https://github.com/salsa-rs/salsa/pull/977))
- Flatten unserializable query dependencies ([#975](https://github.com/salsa-rs/salsa/pull/975))
- optimize `Id::hash` ([#974](https://github.com/salsa-rs/salsa/pull/974))
- Make `thin-vec/serde` dependency dependent on `persistence` feature ([#973](https://github.com/salsa-rs/salsa/pull/973))
- Remove tracked structs from query outputs ([#969](https://github.com/salsa-rs/salsa/pull/969))
- Remove jemalloc ([#972](https://github.com/salsa-rs/salsa/pull/972))
- Initial persistent caching prototype ([#967](https://github.com/salsa-rs/salsa/pull/967))
- Fix `maybe_changed_after` runnaway for fixpoint queries ([#961](https://github.com/salsa-rs/salsa/pull/961))
- add parallel maybe changed after test ([#963](https://github.com/salsa-rs/salsa/pull/963))
- Update tests for Rust 1.89 ([#966](https://github.com/salsa-rs/salsa/pull/966))
- remove allocation lock ([#962](https://github.com/salsa-rs/salsa/pull/962))
- consolidate memory usage information API ([#964](https://github.com/salsa-rs/salsa/pull/964))
- Add heap size support for salsa structs ([#943](https://github.com/salsa-rs/salsa/pull/943))
- Extract the cycle branches from `fetch` and `maybe_changed_after` ([#955](https://github.com/salsa-rs/salsa/pull/955))
- allow reuse of cached provisional memos within the same cycle iteration during `maybe_changed_after` ([#954](https://github.com/salsa-rs/salsa/pull/954))
- Expose API to manually trigger cancellation ([#959](https://github.com/salsa-rs/salsa/pull/959))
- Upgrade dependencies ([#956](https://github.com/salsa-rs/salsa/pull/956))
- Use `CycleHeadSet` in `maybe_update_after` ([#953](https://github.com/salsa-rs/salsa/pull/953))
- Gate accumulator feature behind a feature flag ([#946](https://github.com/salsa-rs/salsa/pull/946))
- optimize allocation fast-path ([#949](https://github.com/salsa-rs/salsa/pull/949))
- remove borrow checks from `ZalsaLocal` ([#939](https://github.com/salsa-rs/salsa/pull/939))
- Do manual trait casting ([#922](https://github.com/salsa-rs/salsa/pull/922))
- Retain backing allocation of `ActiveQuery::input_outputs` in `ActiveQuery::seed_iteration` ([#948](https://github.com/salsa-rs/salsa/pull/948))
- remove extra bounds checks from memo table hot-paths ([#938](https://github.com/salsa-rs/salsa/pull/938))
- Outline all tracing events ([#942](https://github.com/salsa-rs/salsa/pull/942))
- remove bounds and type checks from `IngredientCache` ([#937](https://github.com/salsa-rs/salsa/pull/937))
- Avoid dynamic dispatch to access memo tables ([#941](https://github.com/salsa-rs/salsa/pull/941))
- optimize page access ([#940](https://github.com/salsa-rs/salsa/pull/940))
- Use `inventory` for static ingredient registration ([#934](https://github.com/salsa-rs/salsa/pull/934))
- Fix `heap_size` option not being preserved in tracked impls ([#930](https://github.com/salsa-rs/salsa/pull/930))
- update papaya ([#928](https://github.com/salsa-rs/salsa/pull/928))

## [0.23.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.22.0...salsa-v0.23.0) - 2025-06-27

### Added

- `Update` derive field overwrite support ([#747](https://github.com/salsa-rs/salsa/pull/747))

### Fixed

- fix race in `MemoTableTypes` ([#912](https://github.com/salsa-rs/salsa/pull/912))
- multithreaded nested fixpoint iteration ([#882](https://github.com/salsa-rs/salsa/pull/882))

### Other

- Emit self ty for query debug name of assoc function queries ([#927](https://github.com/salsa-rs/salsa/pull/927))
- Replace ingredient cache with faster ingredient map ([#921](https://github.com/salsa-rs/salsa/pull/921))
- add option to track heap memory usage of memos ([#925](https://github.com/salsa-rs/salsa/pull/925))
- Hide generated structs of tracked functions from docs via `#[doc(hidden)]` ([#917](https://github.com/salsa-rs/salsa/pull/917))
- Add API to dump memory usage ([#916](https://github.com/salsa-rs/salsa/pull/916))
- Revert "Assert size for interned Value" & Mark `Slot` trait as unsafe ([#915](https://github.com/salsa-rs/salsa/pull/915))
- add an option to tune interned garbage collection ([#911](https://github.com/salsa-rs/salsa/pull/911))
- Use explicit discriminants for `QueryOriginKind` for better comparisons ([#913](https://github.com/salsa-rs/salsa/pull/913))
- update boxcar ([#910](https://github.com/salsa-rs/salsa/pull/910))
- use latest revision for dependencies on interned values ([#908](https://github.com/salsa-rs/salsa/pull/908))
- remove high-durability values from interned LRU ([#907](https://github.com/salsa-rs/salsa/pull/907))
- Preserve attributes on interned/tracked struct fields ([#905](https://github.com/salsa-rs/salsa/pull/905))
- Assert size for interned `Value` ([#901](https://github.com/salsa-rs/salsa/pull/901))
- reduce size of interned value metadata ([#903](https://github.com/salsa-rs/salsa/pull/903))
- panic with string message again for cycle panics ([#898](https://github.com/salsa-rs/salsa/pull/898))
- Use `Revision` and `Durability` directly in input `Value` ([#902](https://github.com/salsa-rs/salsa/pull/902))
- Fix flaky parallel_join test ([#900](https://github.com/salsa-rs/salsa/pull/900))
- Bump MSRV to 1.85 ([#899](https://github.com/salsa-rs/salsa/pull/899))
- Simple LRU garbage collection for interned values ([#839](https://github.com/salsa-rs/salsa/pull/839))
- Capture execution backtrace when throwing `UnexpectedCycle` ([#883](https://github.com/salsa-rs/salsa/pull/883))
- Store tracked struct ids as ThinVec on Revisions ([#892](https://github.com/salsa-rs/salsa/pull/892))
- Update dependencies, remove unused `heck` dependency ([#894](https://github.com/salsa-rs/salsa/pull/894))
- Set `validate_final` in `execute` after removing the last cycle head ([#890](https://github.com/salsa-rs/salsa/pull/890))
- Pack `QueryEdge` memory layout ([#886](https://github.com/salsa-rs/salsa/pull/886))
- Lazily allocate extra memo state ([#888](https://github.com/salsa-rs/salsa/pull/888))
- Pack `QueryOrigin` memory layout ([#885](https://github.com/salsa-rs/salsa/pull/885))
- Restrict memo size assertion to 64bit platforms ([#884](https://github.com/salsa-rs/salsa/pull/884))
- Don't report stale outputs if there is newer generation in new_outputs ([#879](https://github.com/salsa-rs/salsa/pull/879))
- Fix hang in nested fixpoint iteration ([#871](https://github.com/salsa-rs/salsa/pull/871))
- Add debug spans for `new_revision` and `evict_lru` ([#881](https://github.com/salsa-rs/salsa/pull/881))
- Add fetch span ([#875](https://github.com/salsa-rs/salsa/pull/875))
- shrink_to_fit `IdentityMap` before storing it ([#816](https://github.com/salsa-rs/salsa/pull/816))
- Allow lifetimes in arguments in tracked fns with >1 parameters ([#880](https://github.com/salsa-rs/salsa/pull/880))
- Replace loom with shuttle ([#876](https://github.com/salsa-rs/salsa/pull/876))
- Use generational identifiers for tracked structs ([#864](https://github.com/salsa-rs/salsa/pull/864))

### Fixed

- `#[doc(hidden)]` auto-generated tracked-fn structs ([#917](https://github.com/salsa-rs/salsa/pull/917))

## [0.22.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.21.1...salsa-v0.22.0) - 2025-05-23

### Fixed

- fix memo table growth condition ([#850](https://github.com/salsa-rs/salsa/pull/850))
- incorrect caching for queries participating in fixpoint ([#843](https://github.com/salsa-rs/salsa/pull/843))
- change detection for fixpoint queries ([#836](https://github.com/salsa-rs/salsa/pull/836))

### Other

- Allow creation of tracked associated functions (without `self`) ([#859](https://github.com/salsa-rs/salsa/pull/859))
- Short-circuit `block-on` if same thread ([#862](https://github.com/salsa-rs/salsa/pull/862))
- Skip release-plz jobs on forks ([#873](https://github.com/salsa-rs/salsa/pull/873))
- Unwind with specific type when encountering an unexpected cycle ([#856](https://github.com/salsa-rs/salsa/pull/856))
- Remove jar mentions from book ([#775](https://github.com/salsa-rs/salsa/pull/775))
- Implement an `!Update` bound escape hatch for tracked fn ([#867](https://github.com/salsa-rs/salsa/pull/867))
- Only enable `boxcar/loom` when `loom` feature is enabled ([#869](https://github.com/salsa-rs/salsa/pull/869))
- Remove default `PartialOrd` and `Ord` derives for salsa-structs ([#868](https://github.com/salsa-rs/salsa/pull/868))
- update boxcar ([#865](https://github.com/salsa-rs/salsa/pull/865))
- speed-up cycle-retry logic ([#861](https://github.com/salsa-rs/salsa/pull/861))
- Fix returns(deref | as_ref | as_deref) in tracked methods ([#857](https://github.com/salsa-rs/salsa/pull/857))
- Changed `return_ref` syntax to `returns(as_ref)` and `returns(cloned)` ([#772](https://github.com/salsa-rs/salsa/pull/772))
- Work around a rust-analyzer bug ([#855](https://github.com/salsa-rs/salsa/pull/855))
- Lazy finalization of cycle participants in `maybe_changed_after` ([#854](https://github.com/salsa-rs/salsa/pull/854))
- Do not re-verify already verified memoized value in cycle verification ([#851](https://github.com/salsa-rs/salsa/pull/851))
- Pass cycle heads as out parameter for `maybe_changed_after` ([#852](https://github.com/salsa-rs/salsa/pull/852))
- Move salsa event system into `Zalsa` ([#849](https://github.com/salsa-rs/salsa/pull/849))
- gate loom dependency under feature flag ([#844](https://github.com/salsa-rs/salsa/pull/844))
- Add loom support ([#842](https://github.com/salsa-rs/salsa/pull/842))
- Clean up some unsafety ([#830](https://github.com/salsa-rs/salsa/pull/830))

## [0.21.1](https://github.com/salsa-rs/salsa/compare/salsa-v0.21.0...salsa-v0.21.1) - 2025-04-30

### Added

- Make `attach` pub ([#832](https://github.com/salsa-rs/salsa/pull/832))

### Other

- better debug name for interned query arguments ([#837](https://github.com/salsa-rs/salsa/pull/837))
- Avoid panic in `Backtrace::capture` if `query_stack` is already borrowed ([#835](https://github.com/salsa-rs/salsa/pull/835))
- Clean up `function::execute` ([#833](https://github.com/salsa-rs/salsa/pull/833))
- Change an `assert!` to `assert_eq!` ([#828](https://github.com/salsa-rs/salsa/pull/828))

## [0.21.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.20.0...salsa-v0.21.0) - 2025-04-29

### Fixed

- Access to tracked-struct that was freed during fixpoint ([#817](https://github.com/salsa-rs/salsa/pull/817))
- correct debug output for tracked fields ([#826](https://github.com/salsa-rs/salsa/pull/826))
- Fix incorrect `values_equal` signature ([#825](https://github.com/salsa-rs/salsa/pull/825))
- allow unused lifetimes in tracked_struct expansion ([#824](https://github.com/salsa-rs/salsa/pull/824))

### Other

- Implement a query stack `Backtrace` analog ([#827](https://github.com/salsa-rs/salsa/pull/827))
- Simplify ID conversions ([#822](https://github.com/salsa-rs/salsa/pull/822))
- Attempt to fix codspeed ([#823](https://github.com/salsa-rs/salsa/pull/823))
- Remove unnecessary `Array` abstraction ([#821](https://github.com/salsa-rs/salsa/pull/821))
- Add a compile-fail test for a `'static` `!Update` struct ([#820](https://github.com/salsa-rs/salsa/pull/820))
- squelch most clippy warnings in generated code ([#809](https://github.com/salsa-rs/salsa/pull/809))
- Include struct name in formatted input-field index ([#819](https://github.com/salsa-rs/salsa/pull/819))
- Force inline `fetch_hot` ([#818](https://github.com/salsa-rs/salsa/pull/818))
- Per ingredient sync table ([#650](https://github.com/salsa-rs/salsa/pull/650))
- Use `DatabaseKey` for interned events ([#813](https://github.com/salsa-rs/salsa/pull/813))
- [refactor] More `fetch_hot` simplification ([#793](https://github.com/salsa-rs/salsa/pull/793))
- Don't store the fields in the interned map ([#812](https://github.com/salsa-rs/salsa/pull/812))
- Fix ci not always running ([#810](https://github.com/salsa-rs/salsa/pull/810))

## [0.20.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.19.0...salsa-v0.20.0) - 2025-04-22

### Added

- Drop `Debug` requirements and flip implementation defaults ([#756](https://github.com/salsa-rs/salsa/pull/756))

### Fixed

- Dereferencing freed memos when verifying provisional memos ([#788](https://github.com/salsa-rs/salsa/pull/788))
- `#[doc(hidden)]` `plumbing` module ([#781](https://github.com/salsa-rs/salsa/pull/781))
- Use `changed_at` revision when updating fields ([#778](https://github.com/salsa-rs/salsa/pull/778))

### Other

- Reduce memory usage by deduplicating type information ([#803](https://github.com/salsa-rs/salsa/pull/803))
- Make interned's `last_interned_at` equal `Revision::MAX` if they are interned outside a query ([#804](https://github.com/salsa-rs/salsa/pull/804))
- Add a third cycle mode, equivalent to old Salsa cycle behavior ([#801](https://github.com/salsa-rs/salsa/pull/801))
- Update compact_str from 0.8 to 0.9 ([#794](https://github.com/salsa-rs/salsa/pull/794))
- Implement `Update` for `ThinVec` ([#807](https://github.com/salsa-rs/salsa/pull/807))
- Don't push an unnecessary active query for `deep_verify_memo` ([#806](https://github.com/salsa-rs/salsa/pull/806))
- Inline/Outline more cold and slow paths ([#805](https://github.com/salsa-rs/salsa/pull/805))
- `#[inline]` some things ([#799](https://github.com/salsa-rs/salsa/pull/799))
- Discard unnecessary atomic load ([#780](https://github.com/salsa-rs/salsa/pull/780))
- Print query stack when encountering unexpected cycle ([#796](https://github.com/salsa-rs/salsa/pull/796))
- Remove incorrect `parallel_scope` API ([#797](https://github.com/salsa-rs/salsa/pull/797))
- [refactor] Simplify `fetch_hot` ([#792](https://github.com/salsa-rs/salsa/pull/792))
- [refactor] Reuse the same stack for all cycles heads in `validate_same_iteration` ([#791](https://github.com/salsa-rs/salsa/pull/791))
- add WillIterateCycle event ([#790](https://github.com/salsa-rs/salsa/pull/790))
- [fix] Use `validate_maybe_provisional` instead of `validate_provisional` ([#789](https://github.com/salsa-rs/salsa/pull/789))
- Use `ThinVec` for `CycleHeads` ([#787](https://github.com/salsa-rs/salsa/pull/787))
- Keep edge condvar on stack instead of allocating it in an `Arc` ([#773](https://github.com/salsa-rs/salsa/pull/773))
- allow reuse of cached provisional memos within the same cycle iteration ([#786](https://github.com/salsa-rs/salsa/pull/786))
- Implement `Lookup`/`HashEqLike` for `Arc` ([#784](https://github.com/salsa-rs/salsa/pull/784))
- Normalize imports style ([#779](https://github.com/salsa-rs/salsa/pull/779))
- Clean up `par_map` a bit ([#742](https://github.com/salsa-rs/salsa/pull/742))
- Fix typo in comment ([#777](https://github.com/salsa-rs/salsa/pull/777))
- Document most safety blocks ([#776](https://github.com/salsa-rs/salsa/pull/776))
- Use html directory for mdbook artifact ([#774](https://github.com/salsa-rs/salsa/pull/774))
- Move `verified_final` from `Memo` into `QueryRevisions` ([#769](https://github.com/salsa-rs/salsa/pull/769))
- Use `ThinVec` for `MemoTable`, halving its size ([#770](https://github.com/salsa-rs/salsa/pull/770))
- Remove unnecessary query stack acess in `block_on` ([#771](https://github.com/salsa-rs/salsa/pull/771))
- Replace memo queue with append-only vector ([#767](https://github.com/salsa-rs/salsa/pull/767))
- update boxcar ([#696](https://github.com/salsa-rs/salsa/pull/696))
- Remove extra page indirection in `Table` ([#710](https://github.com/salsa-rs/salsa/pull/710))
- update release steps ([#705](https://github.com/salsa-rs/salsa/pull/705))
- Remove some unnecessary panicking paths in cycle execution ([#765](https://github.com/salsa-rs/salsa/pull/765))
- *(perf)* Pool `ActiveQuerys` in the query stack ([#629](https://github.com/salsa-rs/salsa/pull/629))
- Resolve unwind safety fixme ([#761](https://github.com/salsa-rs/salsa/pull/761))
- Enable Garbage Collection for Interned Values ([#602](https://github.com/salsa-rs/salsa/pull/602))
- bug [salsa-macros]: Improve debug name of tracked methods ([#755](https://github.com/salsa-rs/salsa/pull/755))
- Remove dead code ([#764](https://github.com/salsa-rs/salsa/pull/764))
- Reduce unnecessary conditional work in `deep_verify_memo` ([#759](https://github.com/salsa-rs/salsa/pull/759))
- Use a `Vec` for `CycleHeads` ([#760](https://github.com/salsa-rs/salsa/pull/760))
- Use nextest for miri test runs ([#758](https://github.com/salsa-rs/salsa/pull/758))
- Pin `half` version to prevent CI failure ([#757](https://github.com/salsa-rs/salsa/pull/757))
- rewrite cycle handling to support fixed-point iteration ([#603](https://github.com/salsa-rs/salsa/pull/603))

## [0.19.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.18.0...salsa-v0.19.0) - 2025-03-10

### Fixed

- fix typo
- fix enums bug

### Other

- Have salsa not depend on salsa-macros ([#750](https://github.com/salsa-rs/salsa/pull/750))
- Group versions of packages together for releases ([#751](https://github.com/salsa-rs/salsa/pull/751))
- use `portable-atomic` in `IngredientCache` to compile on `powerpc-unknown-linux-gnu` ([#749](https://github.com/salsa-rs/salsa/pull/749))
- Store view downcaster in function ingredients directly ([#720](https://github.com/salsa-rs/salsa/pull/720))
- Some small perf things ([#744](https://github.com/salsa-rs/salsa/pull/744))
- :replace instead of std::mem::replace ([#746](https://github.com/salsa-rs/salsa/pull/746))
- Cleanup `Cargo.toml`s ([#745](https://github.com/salsa-rs/salsa/pull/745))
- Drop clone requirement for accumulated values
- implement `Update` trait for `IndexMap`, and `IndexSet`
- more correct bounds on `Send` and `Sync` implementation `DeletedEntries`
- replace `arc-swap` with manual `AtomicPtr`
- Remove unnecessary `current_revision` call from `setup_interned_struct`
- Merge pull request #731 from Veykril/veykril/push-nzkwqzxxkxou
- Remove some dynamically dispatched `Database::event` calls
- Lazy fetching
- Add small supertype input benchmark
- Replace a `DashMap` with `RwLock` as writing is rare for it
- address review comments
- Skip memo ingredient index mapping for non enum tracked functions
- Trade off a bit of memory for more speed in `MemoIngredientIndices`
- Introduce Salsa enums
- Cancel duplicate test workflow runs
- implement `Update` trait for `hashbrown::HashMap`
- Move `unwind_if_revision_cancelled` from `ZalsaLocal` to `Zalsa`
- Don't clone strings in benchmarks
- Merge pull request #714 from Veykril/veykril/push-synxntlkqqsq
- Merge pull request #711 from Veykril/veykril/push-stmmwmtprovt
- Merge pull request #715 from Veykril/veykril/push-plwpsqknwulq
- Enforce `unsafe_op_in_unsafe_fn`
- Remove some `ZalsaDatabase::zalsa` calls
- Remove outdated FIXME
- Replace `IngredientCache` lock with atomic primitive
- Reduce method delegation duplication
- Automatically clear the cancellation flag when cancellation completes
- Allow trigger LRU eviction without increasing the current revision
- Simplify `Ingredient::reset_for_new_revision` setup
- Require mut Zalsa access for setting the lru limit
- Split off revision bumping from `zalsa_mut` access
- Update `hashbrown` (0.15) and `hashlink` (0.10)
