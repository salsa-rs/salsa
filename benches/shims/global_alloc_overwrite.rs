#[cfg(all(
    not(target_os = "windows"),
    not(target_os = "openbsd"),
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64"
    )
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Disable decay after 10s because it can show up as *random* slow allocations
// in benchmarks. We don't need purging in benchmarks because it isn't important
// to give unallocated pages back to the OS.
// https://jemalloc.net/jemalloc.3.html#opt.dirty_decay_ms
#[cfg(all(
    not(target_os = "windows"),
    not(target_os = "openbsd"),
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64"
    )
))]
#[allow(non_upper_case_globals)]
#[export_name = "_rjem_malloc_conf"]
#[allow(unsafe_code)]
pub static _rjem_malloc_conf: &[u8] = b"dirty_decay_ms:-1,muzzy_decay_ms:-1\0";
