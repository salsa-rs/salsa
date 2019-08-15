# 0.13.0

- **Breaking change:** adopt the new `Durability` API proposed in [RFC #6]
    - this replaces and generalizes the existing concepts of constants
- **Breaking change:** remove "volatile" queries
    - instead, create a normal query which invokes the
      `report_untracked_read` method on the salsa runtime
- introduce "slots", an optimization to salsa's internal workings
- document `#[salsa::requires]` attribute, which permits private dependencies
- Adopt `AtomicU64` for `runtimeId` (#182)
- use `ptr::eq` and `ptr::hash` for readability
- upgrade parking lot, rand dependencies

[RFC #6]: https://github.com/salsa-rs/salsa-rfcs/pull/6
