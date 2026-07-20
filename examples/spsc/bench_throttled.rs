//! Throttled-producer offered-load sweep with coordinated-omission correction.
//!
//! Method (Gil Tene, "How NOT to Measure Latency"):
//! - Producer pushes on an open-loop schedule: `schedule_tsc(i) = t_start + i * (f_tsc / lambda)`.
//!   The schedule advances unconditionally. When the consumer stalls, items
//!   queue up; the schedule does NOT pause to wait for them. This is the only
//!   way to surface the true tail under load.
//! - Each item carries both `schedule_tsc` (when it should have been delivered)
//!   and `push_tsc` (when the producer actually published it).
//! - Consumer records `now - schedule_tsc` as USER-PERCEIVED latency. This is
//!   CO-robust: a consumer stall that queues N items shows up as N samples
//!   with steadily growing latency rather than being collapsed into one.
//! - For each offered rate we report a CO-corrected user latency histogram and
//!   a `saturated` flag derived from backpressure + final schedule lag.
//!
//! `BENCH_DEBUG=1` additionally captures SYSTEM latency (`now - push_tsc`),
//! which isolates queue overhead from producer schedule jitter when *not*
//! saturated. It is meaningless once saturated (the queue depth itself is the
//! latency) and is suppressed for any rate flagged saturated.
//!
//! `BENCH_RATES=1000000,10000000,...` overrides the default sweep.
//!
//! Reading list:
//! - Gil Tene, "How NOT to Measure Latency" (Strange Loop 2015).
//! - HdrHistogram (github.com/HdrHistogram/HdrHistogram).
//! - wrk2, jHiccup (Gil Tene's reference tools).
//! - Nitsan Wakart's blog on JCTools queues for the same patterns in Java.
//!
//! Required environment (identical to bench_handoff_under_coherency_contention.rs):
//! - Kernel cmdline:
//!     isolcpus=<P>,<C> nohz_full=<P>,<C> rcu_nocbs=<P>,<C>
//!     intel_idle.max_cstate=0 processor.max_cstate=0
//! - `echo performance > /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`
//! - `echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo`
//! - Offline SMT siblings of the two bench cores (or isolate them too).
//! - Pick two cores that share L3 but are different physical cores (lscpu -e).
//! - Run with `ulimit -l unlimited` (or sudo) so mlockall succeeds.
//! - Run on AC power if on a laptop (SMI rate is higher on battery).
//! - Pass env vars through sudo with `sudo -E env BENCH_DEBUG=1 ...`.
//!
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use std::{env, thread};

use hdrhistogram::Histogram;
use low_latency_data_structures::bench::tsc::{calibrate_hz, rdtscp};
use low_latency_data_structures::bench::{fmt, loc, preflight};
use low_latency_data_structures::mem::global::GlobalAllocator;
use low_latency_data_structures::spsc::{Options, new};

/// Item the producer hands off. 16 bytes -- fits in half a cache line.
/// `schedule_tsc` is the deadline (CO-corrected reference frame).
/// `push_tsc` is the actual publish moment (system reference frame).
#[derive(Clone, Copy)]
struct Stamped {
    schedule_tsc: u64,
    push_tsc: u64,
}

fn preflight(used_cores: &[usize]) {
    let mut r = preflight::PreflightReport::default();
    preflight::release_build(&mut r);
    preflight::cores_online(&mut r, used_cores);
    preflight::cores_isolated(&mut r, used_cores);
    preflight::cores_nohz_full(&mut r, used_cores);
    preflight::cores_performance_governor(&mut r, used_cores);
    preflight::turbo_disabled(&mut r);
    preflight::cores_distinct_physical(&mut r, used_cores);
    preflight::cores_smt_siblings_quiet(&mut r, used_cores);
    preflight::cores_share_l3(&mut r, used_cores);
    preflight::tsc_invariant_and_nonstop(&mut r);
    r.finish();
}

/// Capacity sized to absorb the worst micro-burst from a single LOC tick on
/// nohz_full (rare) without triggering false saturation, while still being
/// small enough that real saturation backs up promptly.
const CAPACITY: usize = 4096;

struct RateResult {
    rate_hz: u64,
    n: u64,
    effective_hz: u64,
    full_pushes: u64,
    final_lag_ns: u64,
    saturated: bool,
    user_hist: Histogram<u64>,
    sys_hist: Option<Histogram<u64>>,
}

fn measure_at_rate(
    rate_hz: u64,
    n: u64,
    warmup: u64,
    tsc_hz: u64,
    producer_core: core_affinity::CoreId,
    consumer_core: core_affinity::CoreId,
    capture_sys: bool,
) -> RateResult {
    let (producer, consumer) = new::<Stamped, CAPACITY, GlobalAllocator>(Options::global_mlocked());
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

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

            // 3 sig figs across the full u64 range covers 1 tick (~0.3ns) up
            // to many seconds. `record()` silently rejects values outside this
            // via the `_ = ` discard if any ever wrap due to rare TSC quirks.
            let mut user_hist = Histogram::<u64>::new(3).unwrap();
            let mut sys_hist = capture_sys.then(|| Histogram::<u64>::new(3).unwrap());
            let mut seen: u64 = 0;
            barrier.wait();
            loop {
                while let Some(Stamped {
                    schedule_tsc,
                    push_tsc,
                }) = consumer.pop()
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
                    // Producer's Release happens-after its last push, so any
                    // items still in flight are visible now. Drain so user-
                    // perceived latency captures backlog left at run end.
                    while let Some(Stamped {
                        schedule_tsc,
                        push_tsc,
                    }) = consumer.pop()
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
            // Open-loop schedule anchor. Advances by dt_q32 every iteration,
            // never pauses for queue/consumer state.
            let mut schedule_q32: u128 = (t_start as u128) << 32;
            let mut full_pushes: u64 = 0;

            for _ in 0..n {
                schedule_q32 += dt_q32;
                let schedule_tsc = (schedule_q32 >> 32) as u64;

                // Wait for the scheduled tick. Re-read tsc every iteration so
                // we don't drift if a stall happened on the previous step.
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
                let mut rejected = producer.push(item);
                let mut was_full = false;
                while let Some(it) = rejected {
                    was_full = true;
                    spin_loop();
                    rejected = producer.push(it);
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

    RateResult {
        rate_hz,
        n,
        effective_hz,
        full_pushes,
        final_lag_ns,
        saturated,
        user_hist,
        sys_hist,
    }
}

fn report_rate(r: &RateResult, tsc_hz: u64) {
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
    // Width 9 fits "1234.56ms"-class strings produced by fmt::ns; bytes ==
    // display columns since fmt::ns is ASCII (`us` not `µs`).
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
}

fn parse_rates(s: &str) -> Vec<u64> {
    s.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid rate in BENCH_RATES: {t:?}"))
        })
        .collect()
}

fn main() {
    let cores = core_affinity::get_core_ids().expect("expected to get list of available cores");
    assert!(
        cores.len() >= 2,
        "need at least 2 separate cores for this benchmark"
    );
    let producer_core = cores[0];
    let consumer_core = cores[1];
    let used_cpu_ids = [producer_core.id, consumer_core.id];
    preflight(&used_cpu_ids);

    unsafe {
        let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
    }

    let tsc_hz = calibrate_hz(Duration::from_millis(200));
    println!("TSC freq: {} Hz ({:.3} GHz)", tsc_hz, tsc_hz as f64 / 1e9);

    let debug = env::var("BENCH_DEBUG").map(|v| v == "1").unwrap_or(false);
    let rates: Vec<u64> = env::var("BENCH_RATES")
        .ok()
        .map(|s| parse_rates(&s))
        .unwrap_or_else(|| {
            vec![
                1_000_000,
                10_000_000,
                28_000_000,
                30_000_000,
                50_000_000,
                100_000_000,
                200_000_000,
                300_000_000,
                500_000_000,
            ]
        });

    // Aim for ~3s of measured wall time per rate, but clamp so the smallest
    // rate stays brief enough and the largest rate produces enough samples
    // for stable p99.99 (~5k items in that bucket at N=50M).
    let target_secs: u64 = 3;
    for &rate in &rates {
        let n = rate
            .saturating_mul(target_secs)
            .clamp(1_000_000, 50_000_000);
        let warmup = n / 10;

        let loc_before = loc::read(&used_cpu_ids);
        let r = measure_at_rate(rate, n, warmup, tsc_hz, producer_core, consumer_core, debug);
        let loc_after = loc::read(&used_cpu_ids);

        report_rate(&r, tsc_hz);
        print!("  LOC delta:");
        for (i, &cpu) in used_cpu_ids.iter().enumerate() {
            match (loc_before[i], loc_after[i]) {
                (Some(b), Some(a)) => print!(" cpu{cpu}=+{}", a.saturating_sub(b)),
                _ => print!(" cpu{cpu}=?"),
            }
        }
        println!();
        println!();
    }
}
