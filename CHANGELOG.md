# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
