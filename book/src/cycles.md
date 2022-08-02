# Cycle handling

By default, when Salsa detects a cycle in the computation graph, Salsa will panic with a [`salsa::Cycle`] as the panic value. The [`salsa::Cycle`] structure that describes the cycle, which can be useful for diagnosing what went wrong.

[`salsa::cycle`]: https://github.com/salsa-rs/salsa/blob/0f9971ad94d5d137f1192fde2b02ccf1d2aca28c/src/lib.rs#L654-L672
