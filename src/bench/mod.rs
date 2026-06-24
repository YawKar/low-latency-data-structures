//! Shared utilities for low-latency benchmarks: composable preflight checks,
//! a rdtscp helper, TSC calibration, and per-cpu local-timer-interrupt
//! counters. Each benchmark composes its own preflight from these pieces -
//! different benchmarks need different guarantees (e.g. throttled load needs
//! invariant TSC, single-thread micros don't).

pub mod fmt;
pub mod loc;
pub mod preflight;
#[cfg(target_arch = "x86_64")]
pub mod tsc;
