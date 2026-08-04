//! Direct one-way handoff latency bench core for SPMC.
//!
//! Runs a single-consumer instantiation. This is the SPMC analogue of the
//! SPSC handoff bench: producer publishes an rdtscp timestamp, the sole
//! consumer reads and records `now - ts`. Extra consumers do not sharpen
//! the handoff picture, they only add fanout cost that belongs in a
//! different bench.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;

use super::{SpmcBench, SpmcCons, SpmcProd, SpmcReadResult};
use crate::bench::harness::TwoCoreCtx;
use crate::bench::harness::report::LatencyReport;
use crate::bench::loc;
use crate::bench::tsc::rdtscp;

/// Tunables for [`run_spmc_handoff`].
#[derive(Clone, Copy, Debug)]
pub struct SpmcHandoffCfg {
    /// Total number of items the producer publishes.
    pub n: u64,
    /// Number of items the consumer discards before recording samples, to
    /// let the pipeline warm the caches and branch predictors.
    pub warmup: u64,
    /// TSC cycles the producer busy-waits after each successful publish,
    /// giving the consumer time to actually observe the value before the
    /// next publish overwrites it (overwriting impls) or before the queue
    /// backpressures (backpressure impls). Without this, overwriting impls
    /// at CAPACITY=1 lap the consumer on every publish and the recorded
    /// distribution reflects only the tiny window where a Value slipped
    /// through, not the true handoff cost. 1000 cycles at ~2.6 GHz is
    /// ~385 ns, matching the pre-harness bench.
    pub producer_pace_cycles: u64,
}

impl Default for SpmcHandoffCfg {
    fn default() -> Self {
        // Match the pre-harness bench for a like-for-like comparison.
        Self {
            n: 10_000_000,
            warmup: 1_000_000,
            producer_pace_cycles: 1000,
        }
    }
}

/// Direct one-way handoff latency across a single-consumer SPMC. Producer
/// publishes an rdtscp timestamp, consumer reads and records
/// `rdtscp() - ts`.
///
/// `Lapped` reads (overwriting impls only) are counted but not recorded:
/// there is no meaningful latency for a message that was never observed.
///
/// Panics if either thread's `set_for_current` / post-pin `sched_getcpu`
/// check fails.
pub fn run_spmc_handoff<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    cfg: SpmcHandoffCfg,
) -> LatencyReport
where
    Q: SpmcBench<u64, CAPACITY>,
{
    let (mut producer, mut consumers) = Q::new(1);
    let mut consumer = consumers.pop().expect("SpmcBench::new(1) returned no cons");
    let cores = ctx.used_cpu_ids();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

    let loc_before = loc::read(&cores);

    let producer_core = ctx.producer_core;
    let consumer_core = ctx.consumer_core;
    let cfg_n = cfg.n;
    let cfg_warmup = cfg.warmup;
    let cfg_pace = cfg.producer_pace_cycles;

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> Histogram<u64> {
            assert!(
                core_affinity::set_for_current(consumer_core),
                "failed to set core affinity for consumer: desired core: {consumer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, consumer_core.id as i32,
                "consumer not pinned where requested"
            );

            let mut hist = Histogram::<u64>::new(3).unwrap();
            let mut seen: u64 = 0;
            barrier.wait();
            loop {
                loop {
                    match consumer.try_read() {
                        SpmcReadResult::Value(ts) => {
                            let now = rdtscp();
                            if seen >= cfg_warmup {
                                // wrapping_sub guards against rare cross-core
                                // TSC skew; record() rejects zero/wraparound
                                // silently via ok().
                                let _ = hist.record(now.wrapping_sub(ts));
                            }
                            seen += 1;
                        }
                        SpmcReadResult::Lapped { .. } => {
                            // Lapped means the producer overwrote at least
                            // one slot before we could read it. No meaningful
                            // latency to record; just tick `seen` so warmup
                            // still advances.
                            seen += 1;
                        }
                        SpmcReadResult::Empty => break,
                    }
                }
                if done.load(Ordering::Acquire) {
                    // Drain anything the producer released before setting
                    // `done` but that we hadn't yet observed.
                    loop {
                        match consumer.try_read() {
                            SpmcReadResult::Value(ts) => {
                                let now = rdtscp();
                                if seen >= cfg_warmup {
                                    let _ = hist.record(now.wrapping_sub(ts));
                                }
                                seen += 1;
                            }
                            SpmcReadResult::Lapped { .. } => {
                                seen += 1;
                            }
                            SpmcReadResult::Empty => return hist,
                        }
                    }
                }
                spin_loop();
            }
        })
    };

    let pthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || {
            assert!(
                core_affinity::set_for_current(producer_core),
                "failed to set core affinity for producer: desired core: {producer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, producer_core.id as i32,
                "producer not pinned where requested"
            );

            barrier.wait();
            for _ in 0..cfg_n {
                // Refresh `ts` on every attempt so the recorded value
                // reflects the moment the slot was actually published, not
                // the moment we first noticed backpressure.
                let published_ts = loop {
                    let ts = rdtscp();
                    if producer.try_publish(ts).is_ok() {
                        break ts;
                    }
                    spin_loop();
                };
                // Pace: give the consumer time to actually read the value
                // before the next publish overwrites it. Essential for
                // overwriting impls at small CAPACITY; benign for backpressure
                // impls (they already self-pace under load).
                if cfg_pace > 0 {
                    let until = published_ts.wrapping_add(cfg_pace);
                    while rdtscp() < until {
                        spin_loop();
                    }
                }
            }
            done.store(true, Ordering::Release);
        })
    };

    barrier.wait();
    pthread.join().unwrap();
    let hist = cthread.join().unwrap();
    let loc_after = loc::read(&cores);

    let loc_delta = cores
        .iter()
        .enumerate()
        .map(|(i, &cpu)| {
            let d = match (loc_before[i], loc_after[i]) {
                (Some(b), Some(a)) => Some(a.saturating_sub(b)),
                _ => None,
            };
            (cpu, d)
        })
        .collect();

    LatencyReport {
        impl_name: Q::NAME,
        warnings: Q::WARNINGS,
        p50: hist.value_at_quantile(0.50),
        p90: hist.value_at_quantile(0.90),
        p99: hist.value_at_quantile(0.99),
        p999: hist.value_at_quantile(0.999),
        max: hist.max(),
        loc_delta,
    }
}
