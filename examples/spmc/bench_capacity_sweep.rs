//! Capacity sweep with a sustained producer. The producer publishes flat
//! out for a fixed wall-clock window. A single consumer reads as fast as
//! it can. We sweep CAPACITY from very small (producer and consumer touch
//! the same cache lines, lots of coherency traffic) to very large (the
//! producer is far ahead, the consumer reads slot lines that the producer
//! wrote long ago and that have likely settled into shared L2 or L3 state
//! already).
//!
//! What this is meant to answer: how does the read-latency distribution
//! and the lap rate change when the producer and consumer working sets
//! overlap versus separate. With packed slots (multiple slots per cache
//! line, the current layout) we expect small CAPACITY to look worst,
//! because every producer write invalidates the cache line that the
//! consumer is about to touch. Large CAPACITY should look better because
//! the producer write and the consumer read land on different cache lines.
//!
//! This is the bench to reach for when arguing about padding Slot to a
//! cache line. Padded slots should flatten this curve. Packed slots should
//! tilt it toward the large-CAPACITY end.
//!
//! Why time-bounded and not item-bounded: a fixed N can finish before the
//! consumer accumulates enough samples on a given capacity, which leaves
//! that row empty. A fixed wall window gives every capacity a fair shot.
//!
//! Why we build the queue on an oversized stack: `[Slot<T>; CAPACITY]` is
//! constructed on the stack inside `new()` before being moved into Arc.
//! At CAPACITY = 1 << 20 the array is 16 MiB, which overflows the default
//! 8 MiB main-thread stack. A short-lived builder thread sidesteps that.
//!
//! `BENCH_RUN_SECS=N` overrides the per-capacity run length (default 2s).
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
//!
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use std::{env, thread};

use duplicate::duplicate;
use hdrhistogram::Histogram;
use low_latency_data_structures::bench::tsc::{calibrate_hz, rdtscp};
use low_latency_data_structures::bench::{fmt, loc, preflight};
use low_latency_data_structures::spmc::consumer::ReadResult;
use low_latency_data_structures::spmc::new;

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

/// Small startup skip per capacity. The first samples can include cache-cold
/// fills and scheduler warmup; everything past this counts.
const WARMUP: u64 = 1000;
/// Plenty of headroom for `[Slot<u64>; 1 << 20]` (~16 MiB) plus everything
/// else the queue construction touches.
const BUILDER_STACK_BYTES: usize = 64 * 1024 * 1024;

struct Out {
    capacity: usize,
    published: u64,
    values: u64,
    lapped: u64,
    value_hist: Histogram<u64>,
}

macro_rules! run_one {
    ($cap:expr, $run_ticks:expr, $p_core:expr, $c_core:expr) => {{
        const CAPACITY: usize = $cap;
        let run_ticks: u64 = $run_ticks;
        let p_core = $p_core;
        let c_core = $c_core;

        // Build the queue on a thread with a generous stack so that very
        // large CAPACITY values do not blow the main stack during the
        // intermediate `[Slot; CAPACITY]` construction inside `new()`.
        let (producer, [mut consumer]) = thread::Builder::new()
            .stack_size(BUILDER_STACK_BYTES)
            .spawn(|| new::<u64, CAPACITY, 1>())
            .unwrap()
            .join()
            .unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let done = Arc::new(AtomicBool::new(false));

        let cthread = {
            let barrier = barrier.clone();
            let done = done.clone();
            thread::spawn(move || -> Out {
                assert!(core_affinity::set_for_current(c_core));
                assert_eq!(unsafe { libc::sched_getcpu() }, c_core.id as i32);
                let mut value_hist = Histogram::<u64>::new(3).unwrap();
                let mut values = 0u64;
                let mut lapped = 0u64;
                let mut seen = 0u64;
                barrier.wait();
                loop {
                    let t0 = rdtscp();
                    match consumer.try_read() {
                        ReadResult::Value(_) => {
                            let dt = rdtscp().wrapping_sub(t0);
                            if seen >= WARMUP {
                                let _ = value_hist.record(dt);
                                values += 1;
                            }
                            seen += 1;
                        }
                        ReadResult::Lapped { .. } => {
                            if seen >= WARMUP {
                                lapped += 1;
                            }
                            seen += 1;
                        }
                        ReadResult::Empty => {
                            if done.load(Ordering::Acquire) {
                                return Out {
                                    capacity: CAPACITY,
                                    published: 0,
                                    values,
                                    lapped,
                                    value_hist,
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
                assert!(core_affinity::set_for_current(p_core));
                assert_eq!(unsafe { libc::sched_getcpu() }, p_core.id as i32);
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
    }};
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
        "TSC freq: {} Hz ({:.3} GHz). run={}s per capacity",
        tsc_hz,
        tsc_hz as f64 / 1e9,
        run_secs,
    );
    println!();
    println!(
        "{:>10} {:>12} {:>11} {:>11} {:>9} {:>10} {:>10} {:>10}",
        "capacity", "published", "values", "lapped", "p50", "p99", "p99.9", "max"
    );

    let loc_before = loc::read(&used_cpu_ids);

    duplicate! {
        [
            CAP;
            [16];
            [256];
            [4096];
            [65536];
            [1048576];
        ]
        {
            let r = run_one!(CAP, run_ticks, producer_core, consumer_core);
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
    }

    let loc_after = loc::read(&used_cpu_ids);
    println!();
    print!("LOC delta:");
    for (i, &cpu) in used_cpu_ids.iter().enumerate() {
        match (loc_before[i], loc_after[i]) {
            (Some(b), Some(a)) => print!(" cpu{cpu}=+{}", a.saturating_sub(b)),
            _ => print!(" cpu{cpu}=?"),
        }
    }
    println!();
}
