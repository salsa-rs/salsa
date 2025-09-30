# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.24.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.23.0...salsa-macros-v0.24.0) - 2025-09-30

### Other

- Initial persistent caching prototype ([#967](https://github.com/salsa-rs/salsa/pull/967))
- Add heap size support for salsa structs ([#943](https://github.com/salsa-rs/salsa/pull/943))
- Upgrade dependencies ([#956](https://github.com/salsa-rs/salsa/pull/956))
- Do manual trait casting ([#922](https://github.com/salsa-rs/salsa/pull/922))
- Avoid dynamic dispatch to access memo tables ([#941](https://github.com/salsa-rs/salsa/pull/941))
- Use `inventory` for static ingredient registration ([#934](https://github.com/salsa-rs/salsa/pull/934))
- Fix `heap_size` option not being preserved in tracked impls ([#930](https://github.com/salsa-rs/salsa/pull/930))

## [0.23.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.22.0...salsa-macros-v0.23.0) - 2025-06-27

### Added

- `Update` derive field overwrite support ([#747](https://github.com/salsa-rs/salsa/pull/747))

### Other

- Emit self ty for query debug name of assoc function queries ([#927](https://github.com/salsa-rs/salsa/pull/927))
- add option to track heap memory usage of memos ([#925](https://github.com/salsa-rs/salsa/pull/925))
- add an option to tune interned garbage collection ([#911](https://github.com/salsa-rs/salsa/pull/911))
- Preserve attributes on interned/tracked struct fields ([#905](https://github.com/salsa-rs/salsa/pull/905))
- Update dependencies, remove unused `heck` dependency ([#894](https://github.com/salsa-rs/salsa/pull/894))
- Allow lifetimes in arguments in tracked fns with >1 parameters ([#880](https://github.com/salsa-rs/salsa/pull/880))

## [0.22.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.21.1...salsa-macros-v0.22.0) - 2025-05-23

### Other

- Allow creation of tracked associated functions (without `self`) ([#859](https://github.com/salsa-rs/salsa/pull/859))
- Implement an `!Update` bound escape hatch for tracked fn ([#867](https://github.com/salsa-rs/salsa/pull/867))
- Fix returns(deref | as_ref | as_deref) in tracked methods ([#857](https://github.com/salsa-rs/salsa/pull/857))
- Changed `return_ref` syntax to `returns(as_ref)` and `returns(cloned)` ([#772](https://github.com/salsa-rs/salsa/pull/772))
- Move salsa event system into `Zalsa` ([#849](https://github.com/salsa-rs/salsa/pull/849))

## [0.21.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.20.0...salsa-macros-v0.21.0) - 2025-04-29

### Fixed

- allow unused lifetimes in tracked_struct expansion ([#824](https://github.com/salsa-rs/salsa/pull/824))

### Other

- Add a compile-fail test for a `'static` `!Update` struct ([#820](https://github.com/salsa-rs/salsa/pull/820))
- squelch most clippy warnings in generated code ([#809](https://github.com/salsa-rs/salsa/pull/809))
- Use `DatabaseKey` for interned events ([#813](https://github.com/salsa-rs/salsa/pull/813))

## [0.20.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.19.0...salsa-macros-v0.20.0) - 2025-04-22

### Added

- Drop `Debug` requirements and flip implementation defaults ([#756](https://github.com/salsa-rs/salsa/pull/756))

### Other

- Add a third cycle mode, equivalent to old Salsa cycle behavior ([#801](https://github.com/salsa-rs/salsa/pull/801))
- Normalize imports style ([#779](https://github.com/salsa-rs/salsa/pull/779))
- Document most safety blocks ([#776](https://github.com/salsa-rs/salsa/pull/776))
- bug [salsa-macros]: Improve debug name of tracked methods ([#755](https://github.com/salsa-rs/salsa/pull/755))
- rewrite cycle handling to support fixed-point iteration ([#603](https://github.com/salsa-rs/salsa/pull/603))

## [0.19.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.18.0...salsa-macros-v0.19.0) - 2025-03-10

### Fixed

- fix enums bug

### Other

- Store view downcaster in function ingredients directly ([#720](https://github.com/salsa-rs/salsa/pull/720))
- :replace instead of std::mem::replace ([#746](https://github.com/salsa-rs/salsa/pull/746))
- Cleanup `Cargo.toml`s ([#745](https://github.com/salsa-rs/salsa/pull/745))
- address review comments
- Skip memo ingredient index mapping for non enum tracked functions
- Trade off a bit of memory for more speed in `MemoIngredientIndices`
- Introduce Salsa enums
- Track revisions for tracked fields only
