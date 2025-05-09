# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.0](https://github.com/salsa-rs/salsa/compare/salsa-v0.21.1...salsa-v0.22.0) - 2025-05-09

### Fixed

- fix memo table growth condition ([#850](https://github.com/salsa-rs/salsa/pull/850))
- incorrect caching for queries participating in fixpoint ([#843](https://github.com/salsa-rs/salsa/pull/843))
- change detection for fixpoint queries ([#836](https://github.com/salsa-rs/salsa/pull/836))

### Other

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
