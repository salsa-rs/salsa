# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.23.0](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.22.0...salsa-macro-rules-v0.23.0) - 2025-06-27

### Added

- `Update` derive field overwrite support ([#747](https://github.com/salsa-rs/salsa/pull/747))

### Other

- Emit self ty for query debug name of assoc function queries ([#927](https://github.com/salsa-rs/salsa/pull/927))
- Replace ingredient cache with faster ingredient map ([#921](https://github.com/salsa-rs/salsa/pull/921))
- add option to track heap memory usage of memos ([#925](https://github.com/salsa-rs/salsa/pull/925))
- Hide generated structs of tracked functions from docs via `#[doc(hidden)]` ([#917](https://github.com/salsa-rs/salsa/pull/917))
- add an option to tune interned garbage collection ([#911](https://github.com/salsa-rs/salsa/pull/911))
- Use explicit discriminants for `QueryOriginKind` for better comparisons ([#913](https://github.com/salsa-rs/salsa/pull/913))
- Preserve attributes on interned/tracked struct fields ([#905](https://github.com/salsa-rs/salsa/pull/905))
- Use `Revision` and `Durability` directly in input `Value` ([#902](https://github.com/salsa-rs/salsa/pull/902))
- Allow lifetimes in arguments in tracked fns with >1 parameters ([#880](https://github.com/salsa-rs/salsa/pull/880))
- Replace loom with shuttle ([#876](https://github.com/salsa-rs/salsa/pull/876))

## [0.22.0](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.21.1...salsa-macro-rules-v0.22.0) - 2025-05-23

### Other

- Allow creation of tracked associated functions (without `self`) ([#859](https://github.com/salsa-rs/salsa/pull/859))
- Remove default `PartialOrd` and `Ord` derives for salsa-structs ([#868](https://github.com/salsa-rs/salsa/pull/868))
- Fix returns(deref | as_ref | as_deref) in tracked methods ([#857](https://github.com/salsa-rs/salsa/pull/857))
- Changed `return_ref` syntax to `returns(as_ref)` and `returns(cloned)` ([#772](https://github.com/salsa-rs/salsa/pull/772))
- Move salsa event system into `Zalsa` ([#849](https://github.com/salsa-rs/salsa/pull/849))
- Add loom support ([#842](https://github.com/salsa-rs/salsa/pull/842))
- Clean up some unsafety ([#830](https://github.com/salsa-rs/salsa/pull/830))

## [0.21.1](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.21.0...salsa-macro-rules-v0.21.1) - 2025-04-30

### Other

- better debug name for interned query arguments ([#837](https://github.com/salsa-rs/salsa/pull/837))

## [0.21.0](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.20.0...salsa-macro-rules-v0.21.0) - 2025-04-29

### Fixed

- correct debug output for tracked fields ([#826](https://github.com/salsa-rs/salsa/pull/826))
- allow unused lifetimes in tracked_struct expansion ([#824](https://github.com/salsa-rs/salsa/pull/824))

### Other

- Implement a query stack `Backtrace` analog ([#827](https://github.com/salsa-rs/salsa/pull/827))
- Simplify ID conversions ([#822](https://github.com/salsa-rs/salsa/pull/822))
- Remove unnecessary `Array` abstraction ([#821](https://github.com/salsa-rs/salsa/pull/821))
- Add a compile-fail test for a `'static` `!Update` struct ([#820](https://github.com/salsa-rs/salsa/pull/820))
- squelch most clippy warnings in generated code ([#809](https://github.com/salsa-rs/salsa/pull/809))

## [0.20.0](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.19.0...salsa-macro-rules-v0.20.0) - 2025-04-22

### Added

- Drop `Debug` requirements and flip implementation defaults ([#756](https://github.com/salsa-rs/salsa/pull/756))

### Other

- Reduce memory usage by deduplicating type information ([#803](https://github.com/salsa-rs/salsa/pull/803))
- Inline/Outline more cold and slow paths ([#805](https://github.com/salsa-rs/salsa/pull/805))
- rewrite cycle handling to support fixed-point iteration ([#603](https://github.com/salsa-rs/salsa/pull/603))

## [0.19.0](https://github.com/salsa-rs/salsa/compare/salsa-macro-rules-v0.18.0...salsa-macro-rules-v0.19.0) - 2025-03-10

### Other

- Store view downcaster in function ingredients directly ([#720](https://github.com/salsa-rs/salsa/pull/720))
