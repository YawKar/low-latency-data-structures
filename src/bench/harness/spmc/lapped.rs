//! Lapped-behaviour sweep for SPMC. Gated on
//! [`SpmcSemantics::OverwritingLapNotified`]: only makes sense against impls
//! that surface lapping to the consumer.
//!
//! The producer publishes flat out for a fixed wall-clock window. The
//! consumer adds a configurable per-read delay (TSC cycles) to provoke
//! lapping. For each delay we record the cost of a `try_read` that returned
//! `Value`, the cost of a `try_read` that returned `Lapped`, the fraction
//! of reads that lapped, and the distribution of skipped counts.
//!
//! We do not report a "recovery latency" (Lapped to next Value time): under
//! sustained producer overflow the consumer is permanently behind, so there
//! is no stable recovery time to report. Per-call Value and Lapped costs
//! plus the lap rate carry the same information without the trap.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;

use super::{SpmcBench, SpmcCons, SpmcProd, SpmcReadResult, SpmcSemantics};
use crate::bench::harness::TwoCoreCtx;
use crate::bench::tsc::rdtscp;

/// Skip a tiny prefix of samples per delay setting to dodge cache-cold and
/// scheduler-warmup noise. Kept small so even the slowest delay setting
/// still records a meaningful histogram inside the run window.
const WARMUP: u64 = 1000;

/// Per-delay outcome from one sweep step.
pub struct LappedRateResult {
    /// Consumer per-read delay in TSC cycles.
    pub delay_cycles: u64,
    /// How many items the producer published during the window.
    pub published: u64,
    /// How many recorded reads returned `Value`.
    pub values: u64,
    /// How many recorded reads returned `Lapped`.
    pub lapped: u64,
    /// Latency of `try_read == Value` calls (TSC cycles).
    pub value_hist: Histogram<u64>,
    /// Latency of `try_read == Lapped` calls (TSC cycles).
    pub lapped_hist: Histogram<u64>,
    /// Distribution of `skipped` counts inside `Lapped` reads.
    pub skipped_hist: Histogram<u64>,
    /// Per-CPU `(cpu, delta)` local timer interrupts during this rate.
    pub loc_delta: Vec<(usize, Option<u64>)>,
}

/// Full sweep report from [`run_spmc_lapped`].
pub struct LappedReport {
    /// Name of the impl (e.g. `"ours"`).
    pub impl_name: &'static str,
    /// Static caveats attached by the adapter.
    pub warnings: &'static [&'static str],
    /// Per-delay outcomes, in the order the delays were swept.
    pub rates: Vec<LappedRateResult>,
}

/// Tunables for [`run_spmc_lapped`].
#[derive(Clone, Debug)]
pub struct SpmcLappedCfg {
    /// Per-read delay values (TSC cycles) to sweep. Default: `[0, 100, 500,
    /// 2_000, 10_000, 50_000, 200_000]`.
    pub delays_cycles: Vec<u64>,
    /// Wall-clock seconds to run each delay step. Default: 2.
    pub run_secs: u64,
}

impl Default for SpmcLappedCfg {
    fn default() -> Self {
        Self {
            delays_cycles: vec![0, 100, 500, 2_000, 10_000, 50_000, 200_000],
            run_secs: 2,
        }
    }
}

/// Run the lapped sweep against `Q`. Returns per-delay latency histograms.
///
/// # Panics
///
/// Panics if `Q::SEMANTICS` is not [`SpmcSemantics::OverwritingLapNotified`].
/// Backpressure impls never lap by construction and would produce all-zero
/// lapped counts, which would be misleading rather than informative.
pub fn run_spmc_lapped<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    cfg: SpmcLappedCfg,
) -> LappedReport
where
    Q: SpmcBench<u64, CAPACITY>,
{
    assert!(
        matches!(Q::SEMANTICS, SpmcSemantics::OverwritingLapNotified),
        "run_spmc_lapped requires OverwritingLapNotified semantics; got {:?}",
        Q::SEMANTICS
    );

    let run_ticks = ctx.tsc_hz * cfg.run_secs;
    let mut out = LappedReport {
        impl_name: Q::NAME,
        warnings: Q::WARNINGS,
        rates: Vec::with_capacity(cfg.delays_cycles.len()),
    };

    for &delay in &cfg.delays_cycles {
        let r = measure::<Q, CAPACITY>(ctx, delay, run_ticks);
        print_rate(&r, ctx);
        out.rates.push(r);
    }

    out
}

fn measure<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    delay_cycles: u64,
    run_ticks: u64,
) -> LappedRateResult
where
    Q: SpmcBench<u64, CAPACITY>,
{
    let (mut producer, mut consumers) = Q::new(1);
    let mut consumer = consumers.pop().expect("SpmcBench::new(1) returned no cons");
    let cores = ctx.used_cpu_ids();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

    let loc_before = crate::bench::loc::read(&cores);

    let producer_core = ctx.producer_core;
    let consumer_core = ctx.consumer_core;

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(
            move || -> (Histogram<u64>, Histogram<u64>, Histogram<u64>, u64, u64) {
                assert!(core_affinity::set_for_current(consumer_core));
                assert_eq!(unsafe { libc::sched_getcpu() }, consumer_core.id as i32);

                let mut value_hist = Histogram::<u64>::new(3).unwrap();
                let mut lapped_hist = Histogram::<u64>::new(3).unwrap();
                let mut skipped_hist = Histogram::<u64>::new(3).unwrap();
                let mut values = 0u64;
                let mut lapped = 0u64;
                let mut seen = 0u64;
                barrier.wait();
                loop {
                    if delay_cycles > 0 {
                        let until = rdtscp().wrapping_add(delay_cycles);
                        while rdtscp() < until {
                            spin_loop();
                        }
                    }
                    let t0 = rdtscp();
                    let r = consumer.try_read();
                    let dt = rdtscp().wrapping_sub(t0);
                    match r {
                        SpmcReadResult::Value(_) => {
                            if seen >= WARMUP {
                                let _ = value_hist.record(dt);
                                values += 1;
                            }
                            seen += 1;
                        }
                        SpmcReadResult::Lapped { skipped } => {
                            if seen >= WARMUP {
                                let _ = lapped_hist.record(dt);
                                let _ = skipped_hist.record(skipped as u64);
                                lapped += 1;
                            }
                            seen += 1;
                        }
                        SpmcReadResult::Empty => {
                            if done.load(Ordering::Acquire) {
                                return (value_hist, lapped_hist, skipped_hist, values, lapped);
                            }
                        }
                    }
                }
            },
        )
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
                // Overwriting impls: try_publish never fails, so no retry loop.
                let _ = producer.try_publish(i);
                i = i.wrapping_add(1);
            }
            done.store(true, Ordering::Release);
            i
        })
    };

    barrier.wait();
    let published = pthread.join().unwrap();
    let (value_hist, lapped_hist, skipped_hist, values, lapped) = cthread.join().unwrap();
    let loc_after = crate::bench::loc::read(&cores);

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

    LappedRateResult {
        delay_cycles,
        published,
        values,
        lapped,
        value_hist,
        lapped_hist,
        skipped_hist,
        loc_delta,
    }
}

fn print_rate(r: &LappedRateResult, ctx: &TwoCoreCtx) {
    use crate::bench::fmt;
    let to_ns = |t: u64| (t as u128 * 1_000_000_000 / ctx.tsc_hz as u128) as u64;
    let total = r.values + r.lapped;
    let lapped_pct = if total > 0 {
        (r.lapped as f64 * 100.0) / total as f64
    } else {
        0.0
    };
    println!();
    println!(
        "delay={:>7} cyc (~{:>8}) published={:>10} values={:>9} lapped={:>9} ({:>5.2}% lapped)",
        r.delay_cycles,
        fmt::ns(to_ns(r.delay_cycles)),
        r.published,
        r.values,
        r.lapped,
        lapped_pct,
    );
    let qn = |h: &Histogram<u64>, p: f64| fmt::ns(to_ns(h.value_at_quantile(p)));
    if r.value_hist.len() > 0 {
        println!(
            "  try_read=Value:  p50={:>9} p99={:>9} p99.9={:>9} max={:>9}",
            qn(&r.value_hist, 0.5),
            qn(&r.value_hist, 0.99),
            qn(&r.value_hist, 0.999),
            fmt::ns(to_ns(r.value_hist.max())),
        );
    }
    if r.lapped_hist.len() > 0 {
        println!(
            "  try_read=Lapped: p50={:>9} p99={:>9} p99.9={:>9} max={:>9}",
            qn(&r.lapped_hist, 0.5),
            qn(&r.lapped_hist, 0.99),
            qn(&r.lapped_hist, 0.999),
            fmt::ns(to_ns(r.lapped_hist.max())),
        );
        let q = |h: &Histogram<u64>, p: f64| h.value_at_quantile(p);
        println!(
            "  skipped:         p50={:>9} p99={:>9} p99.9={:>9} max={:>9}",
            q(&r.skipped_hist, 0.5),
            q(&r.skipped_hist, 0.99),
            q(&r.skipped_hist, 0.999),
            r.skipped_hist.max(),
        );
    }
    print!("  LOC delta:");
    for (cpu, d) in &r.loc_delta {
        match d {
            Some(d) => print!(" cpu{cpu}=+{d}"),
            None => print!(" cpu{cpu}=?"),
        }
    }
    println!();
}
