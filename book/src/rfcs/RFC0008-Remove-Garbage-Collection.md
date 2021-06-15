# Remove garbage collection

## Metadata

* Author: nikomatsakis
* Date: 2021-06-06
* Introduced in: https://github.com/salsa-rs/salsa/pull/267

## Summary

* Remove support for tracing garbage collection
* Make interned keys immortal, for now at least

## Motivation

Salsa has traditionally supported "tracing garbage collection", which allowed the user to remove values that were not used in the most recent revision. While this feature is nice in theory, it is not used in practice. Rust Analyzer, for example, prefers to use the LRU mechanism, which offers stricter limits. Considering that it is not used, supporting the garbage collector involves a decent amount of complexity and makes it harder to experiment with Salsa's structure. Therefore, this RFC proposes to remove support for tracing garbage collection. If desired, it can be added back at some future date in an altered form.

## User's guide

The primary effect for users is that the various 'sweep' methods from the database and queries are removed. The only way to control memory usage in Salsa now is through the LRU mechanisms.

## Reference guide

Removing the GC involves deleting a fair bit of code. The most interesting and subtle code is in the interning support. Previously, interned keys tracked the revision in which they were interned, but also the revision in which they were last accessed. when the sweeping method would run, any interned keys that had not been accessed in the current revision were collected. Since we permitted the GC to run with the read only, we had to be prepared for accesses to interned keys to occur concurrently with the GC, and thus for the possibility that various operations could fail. This complexity is removed, but it means that there is no way to remove interned keys at present.

## Frequently asked questions

### Why not just keep the GC?

The complex.

### Are any users relying on the sweeping functionality?

Hard to say for sure, but none that we know of.

### Don't we want some mechanism to control memory usage?

Yes, but we don't quite know what it looks like. LRU seems to be adequate in practice for present.

### What about for interned keys in particular?

We could add an LRU-like mechanism to interning.
