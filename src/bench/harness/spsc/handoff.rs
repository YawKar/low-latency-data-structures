//! Direct one-way handoff latency bench core.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;

use super::{SpscBench, SpscCons, SpscProd};
use crate::bench::harness::TwoCoreCtx;
use crate::bench::harness::report::LatencyReport;
use crate::bench::loc;
use crate::bench::tsc::rdtscp;

/// Tunables for [`run_spsc_handoff`].
#[derive(Clone, Copy, Debug)]
pub struct SpscHandoffCfg {
    /// Total number of items the producer pushes.
    pub n: u64,
    /// Number of items the consumer discards before recording samples, to
    /// let the pipeline warm the caches and branch predictors.
    pub warmup: u64,
}

impl Default for SpscHandoffCfg {
    fn default() -> Self {
        // Match the pre-harness bench for a like-for-like comparison.
        Self {
            n: 10_000_000,
            warmup: 1_000_000,
        }
    }
}

/// Direct one-way handoff latency: producer pushes an rdtscp timestamp,
/// consumer pops and records `rdtscp() - ts`. Capacity `CAPACITY` should be
/// small (1 for pure handoff, no in-queue residence time).
///
/// Panics if either thread's `set_for_current` / post-pin `sched_getcpu`
/// check fails.
pub fn run_spsc_handoff<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    cfg: SpscHandoffCfg,
) -> LatencyReport
where
    Q: SpscBench<u64, CAPACITY>,
{
    let (mut producer, mut consumer) = Q::new();
    let cores = ctx.used_cpu_ids();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

    let loc_before = loc::read(&cores);

    let producer_core = ctx.producer_core;
    let consumer_core = ctx.consumer_core;
    let cfg_n = cfg.n;
    let cfg_warmup = cfg.warmup;

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
                while let Some(ts) = consumer.try_pop() {
                    let now = rdtscp();
                    if seen >= cfg_warmup {
                        // wrapping_sub guards against rare cross-core TSC
                        // skew; record() rejects zero/wraparound silently.
                        let _ = hist.record(now.wrapping_sub(ts));
                    }
                    seen += 1;
                }
                if done.load(Ordering::Acquire) {
                    // Producer's `done` Release happens-after its last
                    // push, so any items still in flight are visible now.
                    while let Some(ts) = consumer.try_pop() {
                        let now = rdtscp();
                        if seen >= cfg_warmup {
                            let _ = hist.record(now.wrapping_sub(ts));
                        }
                        seen += 1;
                    }
                    break;
                }
                spin_loop();
            }
            hist
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
                // Refresh `ts` on every push attempt so the recorded value
                // reflects the moment the slot was actually published, not
                // the moment we first noticed the queue was full.
                loop {
                    let ts = rdtscp();
                    if producer.try_push(ts).is_ok() {
                        break;
                    }
                    spin_loop();
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
