# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
