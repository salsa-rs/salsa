# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.0](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.21.1...salsa-macros-v0.22.0) - 2025-05-09

### Other

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
