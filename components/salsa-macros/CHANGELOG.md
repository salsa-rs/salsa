# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.18.1](https://github.com/salsa-rs/salsa/compare/salsa-macros-v0.18.0...salsa-macros-v0.18.1) - 2025-02-27

### Fixed

- fix enums bug

### Other

- address review comments
- Skip memo ingredient index mapping for non enum tracked functions
- Trade off a bit of memory for more speed in `MemoIngredientIndices`
- Introduce Salsa enums
- Track revisions for tracked fields only
