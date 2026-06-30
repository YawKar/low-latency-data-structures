//! Lapped behaviour under a sustained producer. The producer publishes flat
//! out for a fixed wall-clock window. The consumer adds a configurable
//! per-read delay to provoke lapping. For each delay we report the cost of
//! a try_read that returned Value, the cost of a try_read that returned
//! Lapped, the Value/Lapped ratio, and the distribution of skipped counts.
//!
//! Why this bench: SPMC broadcast never throttles the producer. A slow
//! consumer is part of the design, so the two interesting questions are how
//! cheap the lap detection is on the happy path and how cheap the catch-up
//! branch is when it triggers. Both are reported as a function of how far
//! behind the consumer runs.
//!
//! We do not report a "recovery latency" (Lapped to next Value time): under
//! sustained producer overflow the consumer is permanently behind, so there
//! is no stable recovery time to report. Per-call Value and Lapped costs
//! plus the lap rate carry the same information without the trap.
//!
//! Why time-bounded and not item-bounded: an item-bounded producer can
//! finish all N publishes inside a few tens of ms (it does not wait for the
//! consumer). A heavily delayed consumer then never accumulates enough
//! samples before the producer signals done. Running the producer for a
//! fixed wall window decouples sample count from producer speed.
//!
//! `BENCH_DELAYS=0,500,5000,...` overrides the default sweep (TSC cycles
//! per consumer iteration). `BENCH_RUN_SECS=N` overrides the per-delay run
//! length (default 2 seconds).
//!
//! Required environment:
//! - Kernel cmdline:
//!     isolcpus=<P>,<C> nohz_full=<P>,<C> rcu_nocbs=<P>,<C>
//!     intel_idle.max_cstate=0 processor.max_cstate=0
//! - `echo performance > /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`
//! - `echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo`
//! - Offline SMT siblings of the two bench cores (or isolate them too).
//! - Pick two cores that share L3 but are different physical cores (lscpu -e).
//! - Run with `ulimit -l unlimited` (or sudo) so mlockall succeeds.
//! - Pass env vars through sudo with `sudo -E env BENCH_DELAYS=... ...`.
//!
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use std::{env, thread};

use hdrhistogram::Histogram;
use low_latency_data_structures::bench::tsc::{calibrate_hz, rdtscp};
use low_latency_data_structures::bench::{fmt, loc, preflight};
use low_latency_data_structures::spmc::{ReadResult, new};

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

/// Small enough that a sluggish consumer laps within a handful of producer
/// publishes, large enough to keep the seq protocol exercised across genuine
/// laps rather than back-to-back same-slot rewrites.
const CAPACITY: usize = 128;
/// Skip a tiny prefix of samples per delay setting to dodge cache-cold and
/// scheduler-warmup noise. Kept small so even the slowest delay setting
/// still records a meaningful histogram inside the run window.
const WARMUP: u64 = 1000;

struct Out {
    published: u64,
    values: u64,
    lapped: u64,
    value_hist: Histogram<u64>,
    lapped_hist: Histogram<u64>,
    skipped_hist: Histogram<u64>,
}

fn measure(
    delay_cycles: u64,
    run_ticks: u64,
    producer_core: core_affinity::CoreId,
    consumer_core: core_affinity::CoreId,
) -> Out {
    let (producer, [mut consumer]) = new::<u64, CAPACITY, 1>();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> Out {
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
                    ReadResult::Value(_) => {
                        if seen >= WARMUP {
                            let _ = value_hist.record(dt);
                            values += 1;
                        }
                        seen += 1;
                    }
                    ReadResult::Lapped { skipped } => {
                        if seen >= WARMUP {
                            let _ = lapped_hist.record(dt);
                            let _ = skipped_hist.record(skipped as u64);
                            lapped += 1;
                        }
                        seen += 1;
                    }
                    ReadResult::Empty => {
                        if done.load(Ordering::Acquire) {
                            return Out {
                                published: 0,
                                values,
                                lapped,
                                value_hist,
                                lapped_hist,
                                skipped_hist,
                            };
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
                producer.publish(i);
                i = i.wrapping_add(1);
            }
            done.store(true, Ordering::Release);
            i
        })
    };

    barrier.wait();
    let published = pthread.join().unwrap();
    let mut out = cthread.join().unwrap();
    out.published = published;
    out
}

fn parse_delays(s: &str) -> Vec<u64> {
    s.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid delay in BENCH_DELAYS: {t:?}"))
        })
        .collect()
}

fn main() {
    let cores = core_affinity::get_core_ids().expect("expected to get list of available cores");
    assert!(cores.len() >= 2, "need at least 2 cores for this benchmark");
    let producer_core = cores[0];
    let consumer_core = cores[1];
    let used_cpu_ids = [producer_core.id, consumer_core.id];
    preflight(&used_cpu_ids);

    unsafe {
        let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
    }

    let tsc_hz = calibrate_hz(Duration::from_millis(200));
    let to_ns = |t: u64| (t as u128 * 1_000_000_000 / tsc_hz as u128) as u64;
    let run_secs: u64 = env::var("BENCH_RUN_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let run_ticks = tsc_hz * run_secs;
    println!(
        "TSC freq: {} Hz ({:.3} GHz). CAPACITY={CAPACITY}, run={}s",
        tsc_hz,
        tsc_hz as f64 / 1e9,
        run_secs,
    );

    let delays: Vec<u64> = env::var("BENCH_DELAYS")
        .ok()
        .map(|s| parse_delays(&s))
        .unwrap_or_else(|| vec![0, 100, 500, 2_000, 10_000, 50_000, 200_000]);

    for &d in &delays {
        println!();
        let loc_before = loc::read(&used_cpu_ids);
        let r = measure(d, run_ticks, producer_core, consumer_core);
        let loc_after = loc::read(&used_cpu_ids);

        let total = r.values + r.lapped;
        let lapped_pct = if total > 0 {
            (r.lapped as f64 * 100.0) / total as f64
        } else {
            0.0
        };
        println!(
            "delay={:>7} cyc (~{:>8}) published={:>10} values={:>9} lapped={:>9} ({:>5.2}% lapped)",
            d,
            fmt::ns(to_ns(d)),
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
        for (i, &cpu) in used_cpu_ids.iter().enumerate() {
            match (loc_before[i], loc_after[i]) {
                (Some(b), Some(a)) => print!(" cpu{cpu}=+{}", a.saturating_sub(b)),
                _ => print!(" cpu{cpu}=?"),
            }
        }
        println!();
    }
}
