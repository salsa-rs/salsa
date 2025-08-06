#![cfg(feature = "inventory")]

mod setup;
mod signal;

mod cycle_a_t1_b_t2;
mod cycle_a_t1_b_t2_fallback;
mod cycle_ab_peeping_c;
mod cycle_nested_deep;
mod cycle_nested_deep_conditional;
mod cycle_nested_deep_conditional_changed;
mod cycle_nested_three_threads;
mod cycle_nested_three_threads_changed;
mod cycle_panic;
mod cycle_provisional_depending_on_itself;
mod parallel_cancellation;
mod parallel_join;
mod parallel_map;

#[cfg(not(feature = "shuttle"))]
pub(crate) mod sync {
    pub use std::sync::*;
    pub use std::thread;

    pub fn check(f: impl Fn() + Send + Sync + 'static) {
        f();
    }
}

#[cfg(feature = "shuttle")]
pub(crate) mod sync {
    pub use shuttle::sync::*;
    pub use shuttle::thread;

    pub fn check(f: impl Fn() + Send + Sync + 'static) {
        shuttle::check_pct(f, 1000, 50);
    }
}

pub(crate) use setup::*;
