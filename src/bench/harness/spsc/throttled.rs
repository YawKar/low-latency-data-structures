//! Throttled-producer offered-load sweep with coordinated-omission
//! correction, as a generic bench core over any [`SpscBench`] adapter.
//!
//! Method (Gil Tene, "How NOT to Measure Latency"):
//! - Producer pushes on an open-loop schedule: `schedule_tsc(i) =
//!   t_start + i * (f_tsc / lambda)`. The schedule advances unconditionally.
//!   When the consumer stalls, items queue up; the schedule does NOT pause
//!   to wait for them. This is the only way to surface the true tail under
//!   load.
//! - Each item carries both `schedule_tsc` (when it should have been
//!   delivered) and `push_tsc` (when the producer actually published it).
//! - Consumer records `now - schedule_tsc` as USER-PERCEIVED latency.
//!   This is CO-robust: a consumer stall that queues N items shows up as
//!   N samples with steadily growing latency rather than being collapsed
//!   into one.
//! - For each offered rate we report a CO-corrected user latency histogram
//!   and a `saturated` flag derived from backpressure + final schedule lag.
//!
//! `capture_sys_latency` additionally captures SYSTEM latency
//! (`now - push_tsc`), which isolates queue overhead from producer schedule
//! jitter when *not* saturated. Meaningless once saturated (the queue depth
//! itself is the latency) and suppressed for any rate flagged saturated.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;

use super::{SpscBench, SpscCons, SpscProd};
use crate::bench::harness::TwoCoreCtx;
use crate::bench::tsc::rdtscp;
use crate::bench::{fmt, loc};

/// Item the throttled bench hands off. 16 bytes -- fits in half a cache
/// line. `schedule_tsc` is the CO-corrected deadline; `push_tsc` is the
/// actual publish moment (system reference frame).
///
/// AnyBitPattern is provided via an `unsafe impl` since the crate does not
/// enable bytemuck's `derive` feature. Both fields are `u64` (AnyBitPattern
/// individually) and there is no padding at `#[repr(C)]`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Stamped {
    /// TSC value at which this item was scheduled to be published.
    pub schedule_tsc: u64,
    /// TSC value at the actual publish moment (rdtscp right before push).
    pub push_tsc: u64,
}

// SAFETY: `Stamped` is `#[repr(C)]` with two `u64` fields and no padding;
// any bit pattern is a valid `Stamped`.
unsafe impl bytemuck::Zeroable for Stamped {}
// SAFETY: same rationale as above.
unsafe impl bytemuck::AnyBitPattern for Stamped {}

/// Tunables for [`run_spsc_throttled`].
#[derive(Clone, Debug)]
pub struct SpscThrottledCfg {
    /// Offered rates in items/sec. One measurement per rate.
    pub rates_hz: Vec<u64>,
    /// Per-rate wall time target; the harness derives `n` from `rate * secs`
    /// clamped into `[min_n, max_n]`.
    pub target_secs: u64,
    /// Warmup fraction as a divisor: `warmup = n / warmup_divisor`.
    /// The default 10 keeps 10 percent of samples as warmup.
    pub warmup_divisor: u64,
    /// If true, also record SYSTEM latency (`now - push_tsc`). Suppressed
    /// per-rate when that rate saturates.
    pub capture_sys_latency: bool,
    /// Lower clamp on `n` (so tiny rates still produce a stable histogram).
    pub min_n: u64,
    /// Upper clamp on `n` (so huge rates do not run forever).
    pub max_n: u64,
}

impl Default for SpscThrottledCfg {
    fn default() -> Self {
        // Match the pre-harness bench for a like-for-like comparison.
        Self {
            rates_hz: vec![
                1_000_000,
                10_000_000,
                28_000_000,
                30_000_000,
                50_000_000,
                100_000_000,
                200_000_000,
                300_000_000,
                500_000_000,
            ],
            target_secs: 3,
            warmup_divisor: 10,
            capture_sys_latency: false,
            min_n: 1_000_000,
            max_n: 50_000_000,
        }
    }
}

/// One rate's worth of measurement.
pub struct ThrottledRateResult {
    /// Offered load in items/sec.
    pub rate_hz: u64,
    /// Actually achieved rate over the measurement window.
    pub effective_hz: u64,
    /// Number of items pushed at this rate.
    pub n: u64,
    /// Number of items whose first push attempt saw a full queue.
    pub full_pushes: u64,
    /// How far behind the ideal schedule the producer was at run end.
    pub final_lag_ns: u64,
    /// True if the run was flagged saturated (see impl for criteria).
    pub saturated: bool,
    /// User-perceived latency histogram (`now - schedule_tsc`), in TSC cycles.
    pub user_hist: Histogram<u64>,
    /// Optional system latency histogram (`now - push_tsc`), in TSC cycles.
    /// `Some` only when `capture_sys_latency` was requested AND the run was
    /// not saturated (system numbers are CO-vulnerable under saturation).
    pub sys_hist: Option<Histogram<u64>>,
    /// Per-CPU LOC counter deltas across this rate's run.
    pub loc_delta: Vec<(usize, Option<u64>)>,
}

/// Aggregate report for one impl across all rates.
pub struct ThrottledReport {
    /// Impl label from `Q::NAME`.
    pub impl_name: &'static str,
    /// Static caveats from `Q::WARNINGS`.
    pub warnings: &'static [&'static str],
    /// One entry per rate in the config, in the order the rates were run.
    pub rates: Vec<ThrottledRateResult>,
}

impl ThrottledReport {
    /// Pretty-print the report in the same layout the pre-harness throttled
    /// bench used, extended with an impl header and warnings.
    pub fn print(&self, tsc_hz: u64) {
        println!("== {} ==", self.impl_name);
        for w in self.warnings {
            println!("  WARN: {w}");
        }
        for r in &self.rates {
            print_rate(r, tsc_hz);
        }
    }
}

fn print_rate(r: &ThrottledRateResult, tsc_hz: u64) {
    let to_ns = |ticks: u64| (ticks as u128 * 1_000_000_000 / tsc_hz as u128) as u64;
    let tag = if r.saturated { " (SATURATED)" } else { "" };
    println!(
        "offered={:>12} eff={:>12} N={:>9} full_pushes={:>9} final_lag={:>9}{}",
        r.rate_hz,
        r.effective_hz,
        r.n,
        r.full_pushes,
        fmt::ns(r.final_lag_ns),
        tag,
    );
    let q = |h: &Histogram<u64>, p: f64| fmt::ns(to_ns(h.value_at_quantile(p)));
    println!(
        "  user-perceived: p50={:>9} p99={:>9} p99.9={:>9} p99.99={:>9} max={:>9}",
        q(&r.user_hist, 0.50),
        q(&r.user_hist, 0.99),
        q(&r.user_hist, 0.999),
        q(&r.user_hist, 0.9999),
        fmt::ns(to_ns(r.user_hist.max())),
    );
    if let Some(h) = r.sys_hist.as_ref() {
        if r.saturated {
            println!("  system latency suppressed: saturated -> CO-vulnerable");
        } else {
            println!(
                "  system [DEBUG]: p50={:>9} p99={:>9} p99.9={:>9} p99.99={:>9} max={:>9}",
                q(h, 0.50),
                q(h, 0.99),
                q(h, 0.999),
                q(h, 0.9999),
                fmt::ns(to_ns(h.max())),
            );
        }
    }
    print!("  LOC delta:");
    for (cpu, d) in &r.loc_delta {
        match d {
            Some(d) => print!(" cpu{cpu}=+{d}"),
            None => print!(" cpu{cpu}=?"),
        }
    }
    println!();
    println!();
}

/// Throttled-producer bench over any `SpscBench<Stamped, CAPACITY>`.
///
/// Runs each rate in `cfg.rates_hz` sequentially, streaming a summary line
/// to stdout as each rate completes, and returns the aggregate report at
/// the end. Streaming lets long sweeps (~30s+) show progress; the returned
/// report is still fully populated so callers can post-process.
pub fn run_spsc_throttled<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    cfg: SpscThrottledCfg,
) -> ThrottledReport
where
    Q: SpscBench<Stamped, CAPACITY>,
{
    println!("== {} ==", Q::NAME);
    for w in Q::WARNINGS {
        println!("  WARN: {w}");
    }

    let mut rates_out = Vec::with_capacity(cfg.rates_hz.len());
    for &rate in &cfg.rates_hz {
        let n = rate
            .saturating_mul(cfg.target_secs)
            .clamp(cfg.min_n, cfg.max_n);
        let warmup = n / cfg.warmup_divisor;

        let r = measure_at_rate::<Q, CAPACITY>(ctx, rate, n, warmup, cfg.capture_sys_latency);
        print_rate(&r, ctx.tsc_hz);
        rates_out.push(r);
    }

    ThrottledReport {
        impl_name: Q::NAME,
        warnings: Q::WARNINGS,
        rates: rates_out,
    }
}

fn measure_at_rate<Q, const CAPACITY: usize>(
    ctx: &TwoCoreCtx,
    rate_hz: u64,
    n: u64,
    warmup: u64,
    capture_sys: bool,
) -> ThrottledRateResult
where
    Q: SpscBench<Stamped, CAPACITY>,
{
    let (mut producer, mut consumer) = Q::new();
    let cores = ctx.used_cpu_ids();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));
    let tsc_hz = ctx.tsc_hz;

    let loc_before = loc::read(&cores);

    let producer_core = ctx.producer_core;
    let consumer_core = ctx.consumer_core;

    // ticks-per-item in Q32 fixed point. ~32 bits of sub-tick precision keeps
    // long runs (10^8 items) from drifting more than a single tick away from
    // the ideal schedule, regardless of integer rounding.
    let dt_q32: u128 = ((tsc_hz as u128) << 32) / (rate_hz as u128);

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> (Histogram<u64>, Option<Histogram<u64>>) {
            assert!(
                core_affinity::set_for_current(consumer_core),
                "failed to set core affinity for consumer: desired core: {consumer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, consumer_core.id as i32,
                "consumer not pinned where requested"
            );

            let mut user_hist = Histogram::<u64>::new(3).unwrap();
            let mut sys_hist = capture_sys.then(|| Histogram::<u64>::new(3).unwrap());
            let mut seen: u64 = 0;
            barrier.wait();
            loop {
                while let Some(Stamped {
                    schedule_tsc,
                    push_tsc,
                }) = consumer.try_pop()
                {
                    let now = rdtscp();
                    if seen >= warmup {
                        let _ = user_hist.record(now.wrapping_sub(schedule_tsc));
                        if let Some(h) = sys_hist.as_mut() {
                            let _ = h.record(now.wrapping_sub(push_tsc));
                        }
                    }
                    seen += 1;
                }
                if done.load(Ordering::Acquire) {
                    while let Some(Stamped {
                        schedule_tsc,
                        push_tsc,
                    }) = consumer.try_pop()
                    {
                        let now = rdtscp();
                        if seen >= warmup {
                            let _ = user_hist.record(now.wrapping_sub(schedule_tsc));
                            if let Some(h) = sys_hist.as_mut() {
                                let _ = h.record(now.wrapping_sub(push_tsc));
                            }
                        }
                        seen += 1;
                    }
                    break;
                }
                spin_loop();
            }
            (user_hist, sys_hist)
        })
    };

    let pthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> (u64, u64, u64) {
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
            let t_start = rdtscp();
            let mut schedule_q32: u128 = (t_start as u128) << 32;
            let mut full_pushes: u64 = 0;

            for _ in 0..n {
                schedule_q32 += dt_q32;
                let schedule_tsc = (schedule_q32 >> 32) as u64;

                loop {
                    let now = rdtscp();
                    if now >= schedule_tsc {
                        break;
                    }
                    spin_loop();
                }

                let push_tsc = rdtscp();
                let item = Stamped {
                    schedule_tsc,
                    push_tsc,
                };
                let mut was_full = false;
                let mut pending = Some(item);
                while let Some(it) = pending {
                    match producer.try_push(it) {
                        Ok(()) => {
                            pending = None;
                        }
                        Err(it) => {
                            was_full = true;
                            spin_loop();
                            pending = Some(it);
                        }
                    }
                }
                if was_full {
                    full_pushes += 1;
                }
            }
            let t_end = rdtscp();
            done.store(true, Ordering::Release);

            let last_schedule = (schedule_q32 >> 32) as u64;
            let final_lag_tsc = t_end.saturating_sub(last_schedule);
            (t_end - t_start, full_pushes, final_lag_tsc)
        })
    };

    barrier.wait();
    let (elapsed_tsc, full_pushes, final_lag_tsc) = pthread.join().unwrap();
    let (user_hist, sys_hist) = cthread.join().unwrap();
    let loc_after = loc::read(&cores);

    let to_ns = |ticks: u64| (ticks as u128 * 1_000_000_000 / tsc_hz as u128) as u64;
    let elapsed_ns = to_ns(elapsed_tsc) as u128;
    let effective_hz = (n as u128 * 1_000_000_000)
        .checked_div(elapsed_ns)
        .unwrap_or(0) as u64;
    let final_lag_ns = to_ns(final_lag_tsc);
    let period_ns = (1_000_000_000u128 / rate_hz as u128).max(1);

    // Saturation: either > 1% of items hit a full queue at least once, or by
    // run end the producer is more than max(10ms, 100 * period) behind ideal.
    // Both signals are conservative; they err toward flagging marginal runs.
    let lag_threshold_ns: u128 = 10_000_000u128.max(100u128 * period_ns);
    let saturated =
        full_pushes.saturating_mul(100) > n || (final_lag_ns as u128) > lag_threshold_ns;

    // Suppress the system-latency histogram once saturated; the "system"
    // frame is meaningless when the queue depth itself dominates.
    let sys_hist = if saturated { None } else { sys_hist };

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

    ThrottledRateResult {
        rate_hz,
        effective_hz,
        n,
        full_pushes,
        final_lag_ns,
        saturated,
        user_hist,
        sys_hist,
        loc_delta,
    }
}
