# Cycle handling

By default, when Salsa detects a cycle in the computation graph, Salsa will panic with a [`salsa::Cycle`] as the panic value. The [`salsa::Cycle`] structure that describes the cycle, which can be useful for diagnosing what went wrong. 
