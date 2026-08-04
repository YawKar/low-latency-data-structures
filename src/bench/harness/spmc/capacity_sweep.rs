//! Capacity sweep for SPMC. A single consumer reads as fast as it can while
//! the producer publishes flat out for a fixed wall-clock window. Reports
//! per-CAPACITY latency percentiles plus counts of `Value` vs `Lapped`
//! reads.
//!
//! Because `CAPACITY` is a const-generic type parameter, each capacity is a
//! distinct monomorphization: the harness exposes a single-capacity entry
//! point [`run_spmc_capacity_sweep_one`] and the caller loops with a macro
//! (`duplicate!`, `seq!`, etc.).
//!
//! Meaning across semantic families:
//! - Overwriting impls (ours): producer never blocks, so latency reflects
//!   coherency traffic and (at small CAPACITY) lap rate.
//! - Backpressure impls (bus): producer is throttled by the consumer's
//!   read rate; the row shows the joint publisher-consumer roundtrip cost
//!   under back-pressure, which is a legitimate "who wins in this scenario"
//!   data point even though the mechanism differs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;

use super::{SpmcBench, SpmcCons, SpmcProd, SpmcReadResult};
use crate::bench::harness::TwoCoreCtx;
use crate::bench::tsc::rdtscp;

/// Small startup skip. The first samples can include cache-cold fills and
/// scheduler warmup; everything past this counts.
const WARMUP: u64 = 1000;
/// Plenty of headroom for `[Slot<u64>; 1 << 20]` (~16 MiB) plus everything
/// else the queue construction touches. Matches the pre-harness bench.
const BUILDER_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Per-capacity outcome from one sweep step.
pub struct CapacitySweepResult {
    /// The capacity that produced this row.
    pub capacity: usize,
    /// How many items the producer published during the window.
    pub published: u64,
    /// How many recorded reads returned `Value`.
    pub values: u64,
    /// How many recorded reads returned `Lapped` (overwriting impls only).
    pub lapped: u64,
    /// Per-value read latency histogram (TSC cycles).
    pub value_hist: Histogram<u64>,
}

/// Tunables for [`run_spmc_capacity_sweep_one`].
#[derive(Clone, Copy, Debug)]
pub struct SpmcCapacitySweepCfg {
    /// Wall-clock seconds to run the capacity. Default: 2.
    pub run_secs: u64,
}

impl Default for SpmcCapacitySweepCfg {
    fn default() -> Self {
        Self { run_secs: 2 }
    }
}

/// Run one capacity of the sweep. Producer publishes for
/// `cfg.run_secs * tsc_hz` TSC cycles; single consumer reads as fast as it
/// can, then joins.
pub fn run_spmc_capacity_sweep_one<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    cfg: SpmcCapacitySweepCfg,
) -> CapacitySweepResult
where
    Q: SpmcBench<u64, CAPACITY>,
{
    let run_ticks = ctx.tsc_hz * cfg.run_secs;

    // Build the queue on a thread with a generous stack: very large CAPACITY
    // values otherwise blow the main stack during the intermediate
    // `[Slot; CAPACITY]` construction inside `Q::new`.
    let (mut producer, mut consumers) = thread::Builder::new()
        .stack_size(BUILDER_STACK_BYTES)
        .spawn(|| Q::new(1))
        .unwrap()
        .join()
        .unwrap();
    let mut consumer = consumers.pop().expect("SpmcBench::new(1) returned no cons");

    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

    let producer_core = ctx.producer_core;
    let consumer_core = ctx.consumer_core;

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> (Histogram<u64>, u64, u64) {
            assert!(core_affinity::set_for_current(consumer_core));
            assert_eq!(unsafe { libc::sched_getcpu() }, consumer_core.id as i32);
            let mut value_hist = Histogram::<u64>::new(3).unwrap();
            let mut values = 0u64;
            let mut lapped = 0u64;
            let mut seen = 0u64;
            barrier.wait();
            loop {
                let t0 = rdtscp();
                match consumer.try_read() {
                    SpmcReadResult::Value(_) => {
                        let dt = rdtscp().wrapping_sub(t0);
                        if seen >= WARMUP {
                            let _ = value_hist.record(dt);
                            values += 1;
                        }
                        seen += 1;
                    }
                    SpmcReadResult::Lapped { .. } => {
                        if seen >= WARMUP {
                            lapped += 1;
                        }
                        seen += 1;
                    }
                    SpmcReadResult::Empty => {
                        if done.load(Ordering::Acquire) {
                            return (value_hist, values, lapped);
                        }
                    }
                }
            }
        })
    };

    let pthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> u64 {
            assert!(core_affinity::set_for_current(producer_core));
            assert_eq!(unsafe { libc::sched_getcpu() }, producer_core.id as i32);
            barrier.wait();
            let stop_at = rdtscp().wrapping_add(run_ticks);
            let mut i = 0u64;
            while rdtscp() < stop_at {
                // Retry on backpressure impls; a no-op for overwriting impls
                // where try_publish always returns Ok.
                while producer.try_publish(i).is_err() {
                    std::hint::spin_loop();
                }
                i = i.wrapping_add(1);
            }
            done.store(true, Ordering::Release);
            i
        })
    };

    barrier.wait();
    let published = pthread.join().unwrap();
    let (value_hist, values, lapped) = cthread.join().unwrap();

    CapacitySweepResult {
        capacity: CAPACITY,
        published,
        values,
        lapped,
        value_hist,
    }
}

/// Convenience printer matching the pre-harness bench's row layout.
pub fn print_capacity_row(r: &CapacitySweepResult, ctx: &TwoCoreCtx) {
    use crate::bench::fmt;
    let to_ns = |t: u64| (t as u128 * 1_000_000_000 / ctx.tsc_hz as u128) as u64;
    println!(
        "{:>10} {:>12} {:>11} {:>11} {:>9} {:>10} {:>10} {:>10}",
        r.capacity,
        r.published,
        r.values,
        r.lapped,
        fmt::ns(to_ns(r.value_hist.value_at_quantile(0.5))),
        fmt::ns(to_ns(r.value_hist.value_at_quantile(0.99))),
        fmt::ns(to_ns(r.value_hist.value_at_quantile(0.999))),
        fmt::ns(to_ns(r.value_hist.max())),
    );
}
