//! x86_64 timestamp counter helpers.

use std::thread;
use std::time::{Duration, Instant};

/// Inline rdtscp. Returns the TSC value with partial serialization (prevents
/// the CPU from speculatively moving the read across surrounding ops).
#[inline(always)]
pub fn rdtscp() -> u64 {
    let mut aux = 0u32;
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) }
}

/// Walltime-vs-TSC calibration. Sleeps for the requested duration on the
/// calling thread, then derives Hz from the TSC delta. Accuracy improves
/// linearly with the sleep duration; 200 ms on an isolated core gets well
/// under 1ppm, far below SMI-class noise.
pub fn calibrate_hz(sleep: Duration) -> u64 {
    for _ in 0..4 {
        let _ = rdtscp();
    }
    let t0 = rdtscp();
    let w0 = Instant::now();
    thread::sleep(sleep);
    let t1 = rdtscp();
    let elapsed_ns = w0.elapsed().as_nanos();
    ((t1 - t0) as u128 * 1_000_000_000 / elapsed_ns) as u64
}
