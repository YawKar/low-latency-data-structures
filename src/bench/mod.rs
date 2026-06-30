//! Shared utilities for low-latency benchmarks: composable preflight checks,
//! a rdtscp helper, TSC calibration, and per-cpu local-timer-interrupt
//! counters. Each benchmark composes its own preflight from these pieces -
//! different benchmarks need different guarantees (e.g. throttled load needs
//! invariant TSC, single-thread micros don't).
//!
//! This module is gated on the internal `_bench_utils` feature, is
//! `#[doc(hidden)]`, and is not part of the public API. It exists so the
//! in-repo `examples/` benchmarks can share preflight code; downstream
//! crates should not rely on it.

#![allow(missing_docs)]

pub mod fmt;
pub mod loc;
pub mod preflight;
#[cfg(target_arch = "x86_64")]
pub mod tsc;
